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
use std::sync::{Arc, Mutex, OnceLock, Weak};

#[cfg(any(feature = "neural", test))]
use slime_core::CandidateRankingRequest;
#[cfg(feature = "neural")]
use slime_core::LiveCandidateRankingSnapshot;
#[cfg(feature = "neural")]
use slime_core::Phase;
use slime_core::{
    CandidateAnnotation, DictionaryPackTrust, DictionaryPackVerificationKey,
    DictionaryPackVersionFloor, EnginePreferences, InputEvent, SlimeAction, SlimeEngine, UserData,
};
#[cfg(feature = "neural")]
use slime_tools::neural::{Rescorer, ScoreRequest, ScoredItem};
#[cfg(feature = "neural")]
use slime_tools::surface_annotation::hiragana_to_katakana;

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

/// `selected` and `selection_start` value for an action without a candidate
/// or segment selection. Swift imports `size_t` as `Int`, where this is `-1`.
pub const NO_SELECTION: usize = usize::MAX;

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
pub const STATUS_INVALID_WEIGHT: u32 = 9;
pub const STATUS_INVALID_COST_GAP: u32 = 10;

#[cfg(any(feature = "neural", test))]
const COST_LOG_SCALE: f64 = 500.0;
#[cfg(any(feature = "neural", test))]
const LIVE_LONG_READING_MIN_CHARACTERS: usize = 4;
#[cfg(feature = "neural")]
const LIVE_NAME_SPELLING_MAX_LAMBDA: f64 = 0.5;
#[cfg(feature = "neural")]
const LIVE_LITERAL_BASE_SWITCH_MARGIN: f64 = 0.2;
#[cfg(feature = "neural")]
const LIVE_LITERAL_BASE_MINIMUM_CHARACTERS: usize = 3;
#[cfg(any(feature = "neural", test))]
const LIVE_DICTIONARY_BASE_FALLBACK_MIN_TARGET_CHARACTERS: usize = 4;
#[cfg(any(feature = "neural", test))]
// Long targets and shorter targets with preceding context can justify scoring
// beyond the normal cost-gap shortcut. Keep the relaxation bounded: an
// unbounded gate introduces plausible short-phrase regressions.
const LIVE_RELAXED_COST_GAP_MIN_TARGET_CHARACTERS: usize = 10;
#[cfg(any(feature = "neural", test))]
const LIVE_LONG_TARGET_MAX_COST_GAP: i32 = 2_000;
#[cfg(any(feature = "neural", test))]
const LIVE_CONTEXTUAL_COST_GAP_MIN_TARGET_CHARACTERS: usize = 6;
#[cfg(any(feature = "neural", test))]
const LIVE_LAMBDA_FALLBACK_MIN_TARGET_CHARACTERS: usize = 10;
#[cfg(feature = "neural")]
const LIVE_LENGTH_CHANGE_SWITCH_MARGIN: f64 = 0.5;
#[cfg(feature = "neural")]
const LIVE_LENGTH_CHANGE_MIN_TARGET_CHARACTERS: usize = 10;
#[cfg(feature = "neural")]
const LIVE_CONTEXTUAL_LENGTH_CHANGE_MIN_TARGET_CHARACTERS: usize = 6;
#[cfg(any(feature = "neural", test))]
const LIVE_DEEP_TAIL_SWITCH_MIN_COST_GAP: i32 = 3_000;
#[cfg(any(feature = "neural", test))]
const LIVE_DEEP_TAIL_SWITCH_MAX_CHARACTERS: usize = 2;
#[cfg(any(feature = "neural", test))]
const LIVE_DEEP_COMPOUND_SWITCH_MIN_COST_GAP: i32 = 1_300;

#[cfg(any(feature = "neural", test))]
#[derive(Clone, Copy, Debug, PartialEq)]
enum SurfaceLengthPolicy {
    Unrestricted,
    Preserve,
    AllowWithMargin(f64),
}

#[cfg(any(feature = "neural", test))]
#[derive(Clone, Copy)]
struct LiveRankingPolicy {
    minimum_switch_margin: f64,
    surface_length: SurfaceLengthPolicy,
    numeric_base_switch_margin: Option<f64>,
    dictionary_base_switch_margin: Option<f64>,
    current_surface_index: Option<usize>,
}

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
    #[cfg(feature = "neural")]
    neural: Option<NeuralRuntime>,
}

#[cfg(feature = "neural")]
#[derive(Clone)]
struct NeuralRuntime {
    rescorer: Arc<Rescorer>,
    lambda: f64,
    max_cost_gap: Option<i32>,
    explicit_max_cost_gap: Option<i32>,
    explicit_confidence: bool,
    explicit_recombination: bool,
    explicit_live_agreement: bool,
    explicit_long_reading_weight: Option<(usize, f64)>,
    explicit_medium_reading_weight: Option<f64>,
}

/// An immutable engine snapshot plus an independently owned model reference.
/// The task may run on a worker while its originating engine continues to
/// process keys; application validates that the engine generation still
/// matches before changing marked text.
#[cfg(feature = "neural")]
pub struct SlimeLiveNeuralTask {
    snapshot: LiveCandidateRankingSnapshot,
    runtime: NeuralRuntime,
    minimum_switch_margin: f64,
    long_reading_minimum_switch_margin: f64,
    numeric_base_switch_margin: f64,
    long_reading_lambda: f64,
    request: Option<CandidateRankingRequest>,
    ranked_surfaces: Option<Vec<String>>,
    retried_suffix_scope: bool,
    evaluation_candidate_expansion: Option<EvaluationCandidateExpansion>,
    original_boundary_snapshot: Option<LiveCandidateRankingSnapshot>,
}

#[cfg(feature = "neural")]
#[derive(Clone, Copy)]
enum EvaluationCandidateExpansion {
    Append {
        fixed_segment_limit: usize,
        recombined_limit: usize,
    },
    GuardedReplace {
        fixed_segment_limit: usize,
        recombined_limit: usize,
    },
}

#[cfg(feature = "neural")]
impl SlimeLiveNeuralTask {
    /// Prepares the optional boundary repair before worker scoring.
    #[doc(hidden)]
    pub fn prepare_particle_boundary_reopen(&mut self) -> bool {
        if self.request.is_some() || self.ranked_surfaces.is_some() {
            return false;
        }
        let original = self.snapshot.clone();
        if self.snapshot.prepare_particle_boundary_reopen() {
            self.original_boundary_snapshot = Some(original);
            true
        } else {
            false
        }
    }

    fn postprocess_request_ranking(
        &self,
        request: &CandidateRankingRequest,
        ranked_surfaces: Option<Vec<String>>,
    ) -> Option<Vec<String>> {
        // A reopened boundary must be supported by the model choice.
        // Dictionary-only tail repair belongs to the original scope;
        // applying it here can turn an unfinished phrase into a compound.
        let ranked_surfaces = if self.original_boundary_snapshot.is_some() {
            prefer_safer_literal_tail(&self.snapshot, request, ranked_surfaces)
        } else {
            postprocess_live_ranking(&self.snapshot, request, ranked_surfaces)
        };
        ranked_surfaces.filter(|ranked| {
            self.original_boundary_snapshot.is_none()
                || ranked.first().is_some_and(|selected| {
                    self.snapshot.boundary_winner_uses_reopened_edge(selected)
                        && self.snapshot.boundary_winner_can_apply(request, selected)
                })
        })
    }

    fn finish_ranked_request(
        &self,
        request: &CandidateRankingRequest,
        ranked_surfaces: Option<Vec<String>>,
        wider_gate_margin: f64,
    ) -> Option<Vec<String>> {
        let ranked = ranked_surfaces.or_else(|| {
            if self.original_boundary_snapshot.is_some() {
                return None;
            }
            (should_use_dictionary_base_after_rejected_neural(
                request,
                self.snapshot.request_target_is_literal(request),
            ) || self
                .snapshot
                .dictionary_base_repairs_specific_literal_tail(request)
                || self
                    .snapshot
                    .dictionary_base_replaces_short_contextual_target(request))
            .then(|| base_ranked_candidate_surfaces(request))
        })?;
        if wider_gate_margin > 0.0 {
            let current = self.snapshot.display_target_surface_for_request(request)?;
            let selected = ranked.first()?;
            if !wider_live_winner_is_useful(
                current,
                selected,
                self.snapshot.request_reopens_stable_prefix(request),
            ) {
                return None;
            }
        }
        Some(ranked)
    }

    fn rank_request(&self, request: &CandidateRankingRequest) -> Option<Vec<String>> {
        let auxiliary_gate = self.snapshot.request_has_verb_auxiliary_candidates(request)
            && base_cost_gap(request).is_some_and(|gap| gap <= 3_500);
        let ordinary_gate = self.original_boundary_snapshot.is_some()
            || should_score_live_neurally(request, self.runtime.max_cost_gap);
        let normal_gate = ordinary_gate || auxiliary_gate;
        let confirm_base =
            !normal_gate && should_confirm_live_contextual_base(&self.snapshot, request);
        if normal_gate || confirm_base {
            // Requests beyond the former ordinary cost gate must overcome
            // a stronger dictionary preference with a larger score margin.
            let wider_gate_margin: f64 = if normal_gate
                && self.original_boundary_snapshot.is_none()
                && self.runtime.max_cost_gap.is_some_and(|maximum| {
                    base_cost_gap(request).is_some_and(|gap| gap > maximum.max(1_500))
                }) {
                0.5
            } else {
                0.0
            };
            let lambda = live_neural_lambda(
                self.runtime.lambda,
                self.long_reading_lambda,
                self.snapshot.resolved_reading(),
            );
            let lambda = if candidate_family_is_name_spelling_ambiguity(request) {
                lambda.min(LIVE_NAME_SPELLING_MAX_LAMBDA)
            } else {
                lambda
            };
            let minimum_switch_margin = live_neural_switch_margin(
                self.minimum_switch_margin,
                self.long_reading_minimum_switch_margin,
                self.snapshot.resolved_reading(),
            );
            let length_policy = if self.original_boundary_snapshot.is_some() {
                SurfaceLengthPolicy::AllowWithMargin(LIVE_LENGTH_CHANGE_SWITCH_MARGIN)
            } else {
                live_surface_length_policy(&self.snapshot, request)
            };
            let ranked_surfaces = self.runtime.rank_live_with_lambda_fallback_after_gate(
                &self.snapshot,
                request,
                lambda,
                self.runtime.lambda,
                LiveRankingPolicy {
                    // Repair an unresolved literal tail without intercepting
                    // the normal retry of a reopened prefix's suffix scope.
                    current_surface_index: (!self.snapshot.request_reopens_stable_prefix(request)
                        && self.snapshot.request_target_has_unresolved_literal_suffix(
                            request,
                            LIVE_LITERAL_BASE_MINIMUM_CHARACTERS,
                        ))
                    .then(|| self.snapshot.display_target_surface_for_request(request))
                    .flatten()
                    .and_then(|surface| {
                        request
                            .candidates
                            .iter()
                            .position(|candidate| candidate.surface == surface)
                    }),
                    minimum_switch_margin: minimum_switch_margin.max(wider_gate_margin),
                    surface_length: length_policy,
                    numeric_base_switch_margin: Some(
                        self.numeric_base_switch_margin.max(wider_gate_margin),
                    ),
                    dictionary_base_switch_margin: (self
                        .snapshot
                        .request_target_has_unresolved_literal_suffix(
                            request,
                            LIVE_LITERAL_BASE_MINIMUM_CHARACTERS,
                        )
                        && !dictionary_base_introduces_implicit_numeric(request))
                    .then_some(LIVE_LITERAL_BASE_SWITCH_MARGIN.max(wider_gate_margin)),
                },
            );
            let ranked_surfaces = self.postprocess_request_ranking(request, ranked_surfaces);
            if confirm_base {
                return ranked_surfaces.filter(|ranked| {
                    ranked.first()
                        == request
                            .candidates
                            .first()
                            .map(|candidate| &candidate.surface)
                });
            }
            self.finish_ranked_request(request, ranked_surfaces, wider_gate_margin)
                .filter(|ranked| {
                    ordinary_gate
                        || confirm_base
                        || ranked.first().is_some_and(|selected| {
                            self.snapshot
                                .verb_auxiliary_repair_is_safe(request, selected)
                        })
                })
        } else if self.snapshot.request_target_is_literal(request)
            || self
                .snapshot
                .dictionary_base_replaces_short_contextual_target(request)
        {
            // The cost-gap gate means the dictionary already has a
            // clear winner, not that delayed LIVE should keep a weaker
            // preview. Reuse the generated context-ranked base order
            // without paying for neural scoring. The apply boundary
            // still checks generation identity and short targets.
            Some(base_ranked_candidate_surfaces(request))
        } else {
            None
        }
    }

    /// Configures an evaluator-only confidence margin for long compositions.
    ///
    /// Product adapters keep the ABI-provided margin for both lengths unless
    /// a separately validated product API is added. Returning `false` means
    /// scoring has already started or `margin` is invalid.
    #[doc(hidden)]
    pub fn evaluation_set_long_reading_minimum_switch_margin(&mut self, margin: f64) -> bool {
        if self.request.is_some()
            || self.ranked_surfaces.is_some()
            || !margin.is_finite()
            || margin < 0.0
        {
            return false;
        }
        self.long_reading_minimum_switch_margin = margin;
        true
    }

    /// Configures evaluator-only candidate expansion before the task runs.
    ///
    /// This Rust-only hook is intentionally absent from the C ABI and product
    /// adapters. Returning `false` means scoring has already started.
    #[doc(hidden)]
    pub fn evaluation_set_candidate_expansion(
        &mut self,
        fixed_segment_limit: usize,
        recombined_limit: usize,
    ) -> bool {
        if self.request.is_some() || self.ranked_surfaces.is_some() {
            return false;
        }
        self.evaluation_candidate_expansion = (fixed_segment_limit > 0 || recombined_limit > 0)
            .then_some(EvaluationCandidateExpansion::Append {
                fixed_segment_limit,
                recombined_limit,
            });
        true
    }

    /// Configures structurally guarded, width-preserving evaluator expansion.
    #[doc(hidden)]
    pub fn evaluation_set_guarded_candidate_expansion(
        &mut self,
        fixed_segment_limit: usize,
        recombined_limit: usize,
    ) -> bool {
        if self.request.is_some() || self.ranked_surfaces.is_some() {
            return false;
        }
        self.evaluation_candidate_expansion = (fixed_segment_limit > 0 || recombined_limit > 0)
            .then_some(EvaluationCandidateExpansion::GuardedReplace {
                fixed_segment_limit,
                recombined_limit,
            });
        true
    }

    fn evaluation_extend_request(&self, request: &mut CandidateRankingRequest) {
        match self.evaluation_candidate_expansion {
            Some(EvaluationCandidateExpansion::Append {
                fixed_segment_limit,
                recombined_limit,
            }) => {
                self.snapshot.evaluation_extend_candidate_ranking_request(
                    request,
                    fixed_segment_limit,
                    recombined_limit,
                );
            }
            Some(EvaluationCandidateExpansion::GuardedReplace {
                fixed_segment_limit,
                recombined_limit,
            }) => {
                self.snapshot
                    .evaluation_guarded_replace_candidate_ranking_request(
                        request,
                        fixed_segment_limit,
                        recombined_limit,
                    );
            }
            None => {}
        }
    }

    /// Copies the effective request for offline diagnosis, including context
    /// and dictionary costs. This Rust-only hook does not run or apply a task.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_request(&self) -> Option<CandidateRankingRequest> {
        self.request
            .clone()
            .or_else(|| self.snapshot.candidate_ranking_request())
    }

    /// Reports the target-specific winner safety check for the dictionary-first
    /// result. This does not cover the complete task application contract.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_dictionary_base_is_safe(&self) -> Option<bool> {
        let request = self.evaluation_request()?;
        let ranked = base_ranked_candidate_surfaces(&request);
        Some(self.snapshot.ranked_winner_is_safe(&request, &ranked))
    }

    /// Runs an additional scoring pass for an explicitly requested offline
    /// trace. This does not change the task's stored ranking or engine state.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_logliks(&self) -> Option<Vec<f64>> {
        let request = self.evaluation_request()?;
        let score_request = ScoreRequest {
            context: request.left_context,
            input_katakana: hiragana_to_katakana(&request.reading),
            candidates: request
                .candidates
                .into_iter()
                .map(|item| item.surface)
                .collect(),
        };
        self.runtime
            .rescorer
            .score_interactive(&score_request)
            .ok()
            .map(|score| score.logliks)
    }

    /// Returns complete surfaces that the captured LIVE worker scope can rank.
    ///
    /// This Rust-only diagnostic hook is intentionally absent from the C ABI;
    /// product adapters must treat the task as opaque and use the apply API.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_rankable_surfaces(&self) -> Vec<String> {
        let request = self
            .request
            .clone()
            .or_else(|| self.snapshot.candidate_ranking_request());
        request
            .as_ref()
            .and_then(|request| self.snapshot.rankable_output_surfaces(request))
            .unwrap_or_default()
    }

    /// Returns complete surfaces in the worker's post-ranking order.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_ranked_surfaces(&self) -> Vec<String> {
        let (Some(request), Some(ranked_surfaces)) =
            (self.request.as_ref(), self.ranked_surfaces.as_ref())
        else {
            return Vec::new();
        };
        let Some(outputs) = self.snapshot.rankable_output_surfaces(request) else {
            return Vec::new();
        };
        ranked_surfaces
            .iter()
            .filter_map(|surface| {
                request
                    .candidates
                    .iter()
                    .position(|candidate| candidate.surface == *surface)
                    .and_then(|index| outputs.get(index).cloned())
            })
            .collect()
    }

    /// Returns the reading covered by this worker's ranking request.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_request_reading(&self) -> Option<String> {
        self.request
            .clone()
            .or_else(|| self.snapshot.candidate_ranking_request())
            .map(|request| request.reading)
    }

    /// Returns the target surface currently shown for this request.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_base_target_surface(&self) -> Option<String> {
        let request = self
            .request
            .clone()
            .or_else(|| self.snapshot.candidate_ranking_request())?;
        self.snapshot
            .evaluation_base_target_surface_for_request(&request)
            .map(str::to_owned)
    }

    /// Whether the request keeps an already stable prefix outside its scope.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_has_stable_prefix(&self) -> bool {
        self.snapshot.evaluation_has_stable_prefix()
    }

    /// Whether the request intentionally reopens a bounded stable prefix.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_reopens_stable_prefix(&self) -> bool {
        let request = self
            .request
            .clone()
            .or_else(|| self.snapshot.candidate_ranking_request());
        request
            .as_ref()
            .is_some_and(|request| self.snapshot.request_reopens_stable_prefix(request))
    }

    /// Whether a rejected bounded full-reading repair fell back to the
    /// original suffix-only ranking scope.
    #[doc(hidden)]
    #[must_use]
    pub fn evaluation_retried_suffix_scope(&self) -> bool {
        self.retried_suffix_scope
    }
}

#[cfg(not(feature = "neural"))]
pub struct SlimeLiveNeuralTask {
    _private: (),
}

#[cfg(feature = "neural")]
struct SharedNeuralModel {
    path: PathBuf,
    rescorer: Weak<Rescorer>,
}

#[cfg(feature = "neural")]
static SHARED_NEURAL_MODEL: OnceLock<Mutex<Option<SharedNeuralModel>>> = OnceLock::new();

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
        #[cfg(feature = "neural")]
        neural: None,
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
            #[cfg(feature = "neural")]
            neural: None,
        })
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
        Some(SlimeHandle {
            engine: SlimeEngine::bundled_with_user_data_and_pack_trust(
                UserData::load(data_dir),
                trust,
            ),
            #[cfg(feature = "neural")]
            neural: None,
        })
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
        Some(SlimeHandle {
            engine: SlimeEngine::bundled_with_user_data_and_pack_trust(
                UserData::load(data_dir),
                trust,
            ),
            #[cfg(feature = "neural")]
            neural: None,
        })
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

/// Loads an optional local neural model used only after the first explicit
/// Space conversion. Loading or scoring failure never changes the base
/// conversion path.
///
/// # Safety
///
/// `handle` must be a live, exclusively accessed pointer returned by an IME
/// creation function. `model_path` must point to `model_path_len` readable
/// UTF-8 bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_enable_neural_reranker(
    handle: *mut SlimeHandle,
    model_path: *const u8,
    model_path_len: usize,
    lambda: f64,
) -> u32 {
    // SAFETY: This function forwards the caller's pointer contract unchanged.
    unsafe { enable_neural_reranker(handle, model_path, model_path_len, lambda, None) }
}

/// Loads an optional local neural model and skips scoring when the base
/// top-two cost gap is greater than `max_cost_gap`.
///
/// The original [`slime_enable_neural_reranker`] remains an ungated ABI for
/// existing callers. A skipped item follows the base conversion path exactly.
///
/// # Safety
///
/// `handle` must be live and exclusively accessed. `model_path` must point to
/// `model_path_len` readable UTF-8 bytes for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_enable_neural_reranker_with_cost_gap(
    handle: *mut SlimeHandle,
    model_path: *const u8,
    model_path_len: usize,
    lambda: f64,
    max_cost_gap: i32,
) -> u32 {
    if max_cost_gap < 0 {
        return STATUS_INVALID_COST_GAP;
    }
    // SAFETY: This function forwards the caller's pointer contract unchanged.
    unsafe {
        enable_neural_reranker(
            handle,
            model_path,
            model_path_len,
            lambda,
            Some(max_cost_gap),
        )
    }
}

/// Overrides the cost-gap gate for explicit Space conversion only.
/// Newly admitted switches must preserve the base's hiragana subsequence.
/// The limit must be nonnegative; loading a model resets the override.
///
/// # Safety
/// `handle` must be a live, exclusively accessed IME handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_explicit_neural_cost_gap(
    handle: *mut SlimeHandle,
    max_cost_gap: i32,
) -> u32 {
    if handle.is_null() {
        return STATUS_NULL_HANDLE;
    }
    if max_cost_gap < 0 {
        return STATUS_INVALID_COST_GAP;
    }
    #[cfg(feature = "neural")]
    {
        // SAFETY: The caller promises exclusive access to a live handle.
        let Some(runtime) = (unsafe { &mut *handle }).neural.as_mut() else {
            return STATUS_NEURAL_UNAVAILABLE;
        };
        runtime.explicit_max_cost_gap = Some(max_cost_gap);
        STATUS_OK
    }
    #[cfg(not(feature = "neural"))]
    {
        STATUS_NEURAL_UNAVAILABLE
    }
}

/// Enables confidence-gated explicit reranking for long readings.
/// Disabled by default; reloading the model resets this option. LIVE is unaffected.
///
/// # Safety
/// `handle` must be a live, exclusively accessed IME handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_explicit_neural_confidence(
    handle: *mut SlimeHandle,
    enabled: bool,
) -> u32 {
    if handle.is_null() {
        return STATUS_NULL_HANDLE;
    }
    #[cfg(feature = "neural")]
    {
        // SAFETY: The caller guarantees exclusive access to a live handle.
        let Some(runtime) = (unsafe { &mut *handle }).neural.as_mut() else {
            return STATUS_NEURAL_UNAVAILABLE;
        };
        runtime.explicit_confidence = enabled;
        STATUS_OK
    }
    #[cfg(not(feature = "neural"))]
    {
        let _ = enabled;
        STATUS_NEURAL_UNAVAILABLE
    }
}

/// Enables the experimental explicit confidence policy for evaluator runs.
///
/// # Safety
/// `handle` must be a live, exclusively accessed IME handle.
#[cfg(feature = "neural")]
#[doc(hidden)]
pub unsafe fn evaluation_set_explicit_confidence(handle: *mut SlimeHandle, enabled: bool) -> bool {
    // SAFETY: The caller guarantees exclusive access to a live handle.
    unsafe { slime_set_explicit_neural_confidence(handle, enabled) == STATUS_OK }
}

/// Enables bounded candidate recombination for experimental evaluator runs.
///
/// # Safety
/// `handle` must be null or a live, exclusively accessed IME handle.
#[cfg(feature = "neural")]
#[doc(hidden)]
pub unsafe fn evaluation_set_explicit_recombination(
    handle: *mut SlimeHandle,
    enabled: bool,
) -> bool {
    // SAFETY: The caller guarantees exclusive access to a live handle if nonnull.
    let Some(handle) = (unsafe { handle.as_mut() }) else {
        return false;
    };
    let Some(runtime) = handle.neural.as_mut() else {
        return false;
    };
    runtime.explicit_recombination = enabled;
    true
}

/// Enables literal-preserving agreement with the current approved LIVE display
/// when starting short explicit conversions. Defaults off; model reload resets it.
///
/// # Safety
/// `handle` must be null or a live, exclusively accessed IME handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_explicit_live_agreement(
    handle: *mut SlimeHandle,
    enabled: bool,
) -> u32 {
    if handle.is_null() {
        return STATUS_NULL_HANDLE;
    }
    #[cfg(feature = "neural")]
    {
        // SAFETY: The caller guarantees exclusive access to a live handle.
        let Some(runtime) = (unsafe { &mut *handle }).neural.as_mut() else {
            return STATUS_NEURAL_UNAVAILABLE;
        };
        runtime.explicit_live_agreement = enabled;
        STATUS_OK
    }
    #[cfg(not(feature = "neural"))]
    {
        let _ = enabled;
        STATUS_NEURAL_UNAVAILABLE
    }
}

/// Enables experimental LIVE-to-Space agreement for evaluator runs.
///
/// # Safety
/// `handle` must be null or a live, exclusively accessed IME handle.
#[cfg(feature = "neural")]
#[doc(hidden)]
pub unsafe fn evaluation_set_explicit_live_agreement(
    handle: *mut SlimeHandle,
    enabled: bool,
) -> bool {
    // SAFETY: The caller guarantees exclusive access to a live handle if nonnull.
    unsafe { slime_set_explicit_live_agreement(handle, enabled) == STATUS_OK }
}

/// Sets an optional weight for long explicit conversions without changing LIVE
/// ranking. A zero minimum disables the override. Enabling a new model resets it.
///
/// # Safety
/// `handle` must be a live, exclusively accessed IME handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_explicit_neural_long_reading_weight(
    handle: *mut SlimeHandle,
    minimum_reading_characters: usize,
    lambda: f64,
) -> u32 {
    if handle.is_null() {
        return STATUS_NULL_HANDLE;
    }
    if !lambda.is_finite() || !(0.0..=1.0).contains(&lambda) {
        return STATUS_INVALID_WEIGHT;
    }
    #[cfg(feature = "neural")]
    {
        // SAFETY: The caller promises exclusive access to a live handle.
        let Some(runtime) = (unsafe { &mut *handle }).neural.as_mut() else {
            return STATUS_NEURAL_UNAVAILABLE;
        };
        runtime.explicit_long_reading_weight =
            (minimum_reading_characters > 0).then_some((minimum_reading_characters, lambda));
        STATUS_OK
    }
    #[cfg(not(feature = "neural"))]
    {
        let _ = minimum_reading_characters;
        STATUS_NEURAL_UNAVAILABLE
    }
}

/// Overrides explicit conversion weight for readings of 3 through 19 Unicode
/// characters. A matching long-reading override takes precedence. Reloading a
/// model resets this override. LIVE weights remain unchanged.
///
/// # Safety
/// `handle` must be a live, exclusively accessed IME handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_explicit_neural_medium_reading_weight(
    handle: *mut SlimeHandle,
    lambda: f64,
) -> u32 {
    if handle.is_null() {
        return STATUS_NULL_HANDLE;
    }
    if !lambda.is_finite() || !(0.0..=1.0).contains(&lambda) {
        return STATUS_INVALID_WEIGHT;
    }
    #[cfg(feature = "neural")]
    {
        // SAFETY: The caller promises exclusive access to a live handle.
        let Some(runtime) = (unsafe { &mut *handle }).neural.as_mut() else {
            return STATUS_NEURAL_UNAVAILABLE;
        };
        runtime.explicit_medium_reading_weight = Some(lambda);
        STATUS_OK
    }
    #[cfg(not(feature = "neural"))]
    {
        STATUS_NEURAL_UNAVAILABLE
    }
}

unsafe fn enable_neural_reranker(
    handle: *mut SlimeHandle,
    model_path: *const u8,
    model_path_len: usize,
    lambda: f64,
    max_cost_gap: Option<i32>,
) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return STATUS_NULL_HANDLE;
        }
        if !lambda.is_finite() || !(0.0..=1.0).contains(&lambda) {
            return STATUS_INVALID_WEIGHT;
        }
        // SAFETY: The caller promises readable bytes for this call.
        let Some(model_path) = (unsafe { decode_utf8_argument(model_path, model_path_len) }) else {
            return STATUS_INVALID_UTF8;
        };

        #[cfg(feature = "neural")]
        {
            let Ok(rescorer) = shared_neural_rescorer(Path::new(model_path)) else {
                return STATUS_NEURAL_LOAD_FAILED;
            };
            // SAFETY: The caller promises a live, exclusively accessed handle.
            unsafe { &mut *handle }.neural = Some(NeuralRuntime {
                rescorer,
                lambda,
                max_cost_gap,
                explicit_max_cost_gap: None,
                explicit_confidence: false,
                explicit_recombination: false,
                explicit_live_agreement: false,
                explicit_long_reading_weight: None,
                explicit_medium_reading_weight: None,
            });
            STATUS_OK
        }
        #[cfg(not(feature = "neural"))]
        {
            let _ = (model_path, max_cost_gap);
            STATUS_NEURAL_UNAVAILABLE
        }
    }));
    result.unwrap_or(STATUS_PANIC)
}

/// Configures whether the adapter will schedule delayed LIVE neural tasks.
///
/// This is separate from model loading because some adapters use the model
/// only after explicit Space conversion. Enabling it without a loaded neural
/// runtime is rejected, while disabling it is always safe.
///
/// # Safety
///
/// `handle` must be live and exclusively accessed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_set_live_neural_ranking_enabled(
    handle: *mut SlimeHandle,
    enabled: bool,
) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() {
            return STATUS_NULL_HANDLE;
        }
        // SAFETY: The caller promises a live, exclusively accessed handle.
        let handle = unsafe { &mut *handle };
        #[cfg(feature = "neural")]
        if enabled && handle.neural.is_none() {
            return STATUS_NEURAL_UNAVAILABLE;
        }
        #[cfg(not(feature = "neural"))]
        if enabled {
            return STATUS_NEURAL_UNAVAILABLE;
        }
        handle.engine.set_delayed_live_ranking_available(enabled);
        STATUS_OK
    }));
    result.unwrap_or(STATUS_PANIC)
}

/// Captures a cheap immutable LIVE snapshot for delayed worker ranking.
///
/// Candidate generation and neural scoring are deferred to
/// [`slime_live_neural_task_run`]. A null result means that LIVE ranking is
/// disabled, private, protected by an explicit choice, or not currently
/// applicable. The returned task never borrows `handle` and must be destroyed
/// with [`slime_live_neural_task_destroy`].
///
/// # Safety
///
/// `handle` must be a live pointer and must not be mutated concurrently during
/// this short snapshot call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_live_neural_task_create(
    handle: *const SlimeHandle,
    minimum_switch_margin: f64,
    numeric_base_switch_margin: f64,
    long_reading_lambda: f64,
) -> *mut SlimeLiveNeuralTask {
    // SAFETY: This compatibility entry point forwards the caller's pointer
    // contract and preserves the original single-margin behavior.
    unsafe {
        slime_live_neural_task_create_v2(
            handle,
            minimum_switch_margin,
            minimum_switch_margin,
            numeric_base_switch_margin,
            long_reading_lambda,
        )
    }
}

/// Captures a delayed LIVE task with a separate confidence margin for
/// readings of four or more characters.
///
/// # Safety
///
/// `handle` must be a live pointer and must not be mutated concurrently during
/// this short snapshot call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_live_neural_task_create_v2(
    handle: *const SlimeHandle,
    minimum_switch_margin: f64,
    long_reading_minimum_switch_margin: f64,
    numeric_base_switch_margin: f64,
    long_reading_lambda: f64,
) -> *mut SlimeLiveNeuralTask {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null()
            || !minimum_switch_margin.is_finite()
            || minimum_switch_margin < 0.0
            || !long_reading_minimum_switch_margin.is_finite()
            || long_reading_minimum_switch_margin < 0.0
            || !numeric_base_switch_margin.is_finite()
            || numeric_base_switch_margin < 0.0
            || !long_reading_lambda.is_finite()
            || !(0.0..=1.0).contains(&long_reading_lambda)
        {
            return ptr::null_mut();
        }
        #[cfg(feature = "neural")]
        {
            // SAFETY: The caller promises a live handle that is not being
            // mutated concurrently for this snapshot operation.
            let handle = unsafe { &*handle };
            let Some(runtime) = handle.neural.clone() else {
                return ptr::null_mut();
            };
            let Some(snapshot) = handle.engine.live_candidate_ranking_snapshot() else {
                return ptr::null_mut();
            };
            Box::into_raw(Box::new(SlimeLiveNeuralTask {
                snapshot,
                runtime,
                minimum_switch_margin,
                long_reading_minimum_switch_margin,
                numeric_base_switch_margin,
                long_reading_lambda,
                request: None,
                ranked_surfaces: None,
                retried_suffix_scope: false,
                evaluation_candidate_expansion: None,
                original_boundary_snapshot: None,
            }))
        }
        #[cfg(not(feature = "neural"))]
        {
            ptr::null_mut()
        }
    }));
    result.unwrap_or(ptr::null_mut())
}

/// Returns the number of Unicode scalar values in the complete reading
/// captured by a delayed LIVE task.
///
/// This is snapshot metadata only. It does not generate candidates, run the
/// model, or access the originating engine. A null task returns zero.
///
/// # Safety
///
/// `task` must be null or a live task pointer returned by
/// [`slime_live_neural_task_create`]. The task must not be destroyed or run
/// concurrently: [`slime_live_neural_task_run`] mutates the snapshot this reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_live_neural_task_reading_character_count(
    task: *const SlimeLiveNeuralTask,
) -> usize {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if task.is_null() {
            return 0;
        }
        #[cfg(feature = "neural")]
        {
            // SAFETY: The caller promises a live task for this immutable read.
            let task = unsafe { &*task };
            task.snapshot.resolved_reading().chars().count()
        }
        #[cfg(not(feature = "neural"))]
        {
            0
        }
    }));
    result.unwrap_or(0)
}

/// Runs candidate generation and neural scoring for an independent LIVE task.
///
/// This is the expensive worker-thread operation. It never accesses the
/// originating engine. The same task must not be run or destroyed
/// concurrently and can be run successfully at most once.
///
/// # Safety
///
/// `task` must be null or a live, exclusively accessed task pointer returned
/// by [`slime_live_neural_task_create`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_live_neural_task_run(task: *mut SlimeLiveNeuralTask) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if task.is_null() {
            return STATUS_NULL_HANDLE;
        }
        #[cfg(feature = "neural")]
        {
            // SAFETY: The caller promises exclusive task access.
            let task = unsafe { &mut *task };
            if task.request.is_some() || task.ranked_surfaces.is_some() {
                return STATUS_INVALID_CANDIDATE;
            }
            task.prepare_particle_boundary_reopen();
            let Some(mut request) = task.snapshot.candidate_ranking_request() else {
                return STATUS_INVALID_CANDIDATE;
            };
            task.evaluation_extend_request(&mut request);
            let mut ranked_surfaces = task.rank_request(&request);
            if ranked_surfaces.is_none()
                && let Some(original) = task.original_boundary_snapshot.take()
            {
                task.snapshot = original;
                let Some(original_request) = task.snapshot.candidate_ranking_request() else {
                    return STATUS_INVALID_CANDIDATE;
                };
                request = original_request;
                task.evaluation_extend_request(&mut request);
                ranked_surfaces = task.rank_request(&request);
            }
            if ranked_surfaces.is_none()
                && task.snapshot.request_reopens_stable_prefix(&request)
                && let Some(suffix_request) =
                    task.snapshot.candidate_ranking_request_without_reopening()
            {
                task.retried_suffix_scope = true;
                request = suffix_request;
                task.evaluation_extend_request(&mut request);
                ranked_surfaces = task.rank_request(&request);
            }
            let Some(ranked_surfaces) = ranked_surfaces else {
                return STATUS_INVALID_CANDIDATE;
            };
            task.snapshot
                .prepare_ranked_prefix_validation(&request, &ranked_surfaces);
            task.request = Some(request);
            task.ranked_surfaces = Some(ranked_surfaces);
            STATUS_OK
        }
        #[cfg(not(feature = "neural"))]
        {
            STATUS_NEURAL_UNAVAILABLE
        }
    }));
    result.unwrap_or(STATUS_PANIC)
}

#[cfg(feature = "neural")]
fn postprocess_live_ranking(
    snapshot: &LiveCandidateRankingSnapshot,
    request: &CandidateRankingRequest,
    ranked_surfaces: Option<Vec<String>>,
) -> Option<Vec<String>> {
    let ranked_surfaces = prefer_safer_literal_tail(snapshot, request, ranked_surfaces);
    prefer_specific_dictionary_repair(snapshot, request, ranked_surfaces)
}

#[cfg(feature = "neural")]
fn prefer_safer_literal_tail(
    snapshot: &LiveCandidateRankingSnapshot,
    request: &CandidateRankingRequest,
    ranked_surfaces: Option<Vec<String>>,
) -> Option<Vec<String>> {
    let mut ranked_surfaces = ranked_surfaces?;
    let Some(alternative) =
        snapshot.safer_literal_tail_for_ranked_winner(request, &ranked_surfaces)
    else {
        return Some(ranked_surfaces);
    };
    let Some(index) = ranked_surfaces
        .iter()
        .position(|surface| surface == &alternative)
    else {
        return Some(ranked_surfaces);
    };
    let alternative = ranked_surfaces.remove(index);
    ranked_surfaces.insert(0, alternative);
    Some(ranked_surfaces)
}

#[cfg(feature = "neural")]
fn prefer_specific_dictionary_repair(
    snapshot: &LiveCandidateRankingSnapshot,
    request: &CandidateRankingRequest,
    ranked_surfaces: Option<Vec<String>>,
) -> Option<Vec<String>> {
    if snapshot.dictionary_base_repairs_specific_literal_tail(request)
        && ranked_surfaces.as_ref().is_some_and(|ranked| {
            snapshot.ranked_winner_keeps_display_target(request, ranked)
                || !snapshot.ranked_winner_is_safe(request, ranked)
        })
    {
        Some(base_ranked_candidate_surfaces(request))
    } else {
        ranked_surfaces
    }
}

#[cfg(any(feature = "neural", test))]
fn should_use_dictionary_base_after_rejected_neural(
    request: &CandidateRankingRequest,
    target_is_literal: bool,
) -> bool {
    if !target_is_literal {
        return false;
    }
    let reading_characters = request.reading.chars().count();
    let unprotected_long_literal = reading_characters
        >= LIVE_DICTIONARY_BASE_FALLBACK_MIN_TARGET_CHARACTERS
        && !request
            .candidates
            .iter()
            .any(|candidate| candidate.surface == request.reading && candidate.cost < i32::MAX);
    let preserves_multi_kana_suffix = reading_characters >= 3
        && dictionary_base_preserves_hiragana_suffix(request)
        && !dictionary_base_introduces_implicit_numeric(request);
    unprotected_long_literal || preserves_multi_kana_suffix
}

#[cfg(any(feature = "neural", test))]
fn dictionary_base_preserves_hiragana_suffix(request: &CandidateRankingRequest) -> bool {
    let Some(base) = request.candidates.first() else {
        return false;
    };
    let base_characters = base.surface.chars().count();
    let reading_characters = request.reading.chars().count();
    let shared_hiragana_suffix = base
        .surface
        .chars()
        .rev()
        .zip(request.reading.chars().rev())
        .take_while(|(surface, reading)| {
            surface == reading && matches!(surface, '\u{3041}'..='\u{3096}' | 'ー' | 'ゝ' | 'ゞ')
        })
        .count();
    let converted_prefix_kanji = base
        .surface
        .chars()
        .take(base_characters.saturating_sub(shared_hiragana_suffix))
        .filter(|character| {
            matches!(
                character,
                '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
            )
        })
        .count();
    let converted_prefix_is_all_kanji = base
        .surface
        .chars()
        .take(base_characters.saturating_sub(shared_hiragana_suffix))
        .all(|character| {
            matches!(
                character,
                '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
            )
        });
    shared_hiragana_suffix >= 2
        && converted_prefix_kanji > 0
        && (base_characters == reading_characters
            || (shared_hiragana_suffix >= 3
                && converted_prefix_kanji >= 2
                && converted_prefix_is_all_kanji))
}

/// Applies a completed LIVE task and visits any resulting typed actions.
///
/// Stale tasks are a successful no-op. All engine state, candidate identity,
/// private-mode state, input reading, stable prefix, and left context are
/// revalidated before marked text can change.
///
/// # Safety
///
/// `handle` and `task` must be live and exclusively accessed for this call.
/// `task` must have completed its worker run, and `callback` follows the same
/// borrowing rules as [`slime_process_actions_v2`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_live_neural_task_apply_actions_v2(
    handle: *mut SlimeHandle,
    task: *const SlimeLiveNeuralTask,
    context: *mut c_void,
    callback: Option<SlimeActionCallbackV2>,
) -> u32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if handle.is_null() || task.is_null() {
            return STATUS_NULL_HANDLE;
        }
        let Some(callback) = callback else {
            return STATUS_NULL_CALLBACK;
        };
        #[cfg(feature = "neural")]
        {
            // SAFETY: The caller promises live, exclusively accessed values.
            let handle = unsafe { &mut *handle };
            let task = unsafe { &*task };
            let (Some(request), Some(ranked_surfaces)) =
                (task.request.as_ref(), task.ranked_surfaces.as_ref())
            else {
                return STATUS_INVALID_CANDIDATE;
            };
            if let Some(actions) =
                handle
                    .engine
                    .apply_live_candidate_ranking(&task.snapshot, request, ranked_surfaces)
            {
                for action in &actions {
                    visit_action_v2(action, context, callback);
                }
            }
            STATUS_OK
        }
        #[cfg(not(feature = "neural"))]
        {
            let _ = (context, callback);
            STATUS_NEURAL_UNAVAILABLE
        }
    }));
    result.unwrap_or(STATUS_PANIC)
}

/// Destroys a LIVE neural task. A null pointer is accepted.
///
/// # Safety
///
/// `task` must be null or an owned live pointer returned by
/// [`slime_live_neural_task_create`] and must not be used after this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn slime_live_neural_task_destroy(task: *mut SlimeLiveNeuralTask) {
    if !task.is_null() {
        // SAFETY: The caller promises ownership of this task pointer.
        drop(unsafe { Box::from_raw(task) });
    }
}

#[cfg(feature = "neural")]
fn shared_neural_rescorer(model_path: &Path) -> Result<Arc<Rescorer>, String> {
    let cache = SHARED_NEURAL_MODEL.get_or_init(|| Mutex::new(None));
    let mut cached = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(shared) = cached.as_ref()
        && shared.path == model_path
        && let Some(rescorer) = shared.rescorer.upgrade()
    {
        return Ok(rescorer);
    }

    let rescorer = Arc::new(Rescorer::load(model_path)?);
    *cached = Some(SharedNeuralModel {
        path: model_path.to_owned(),
        rescorer: Arc::downgrade(&rescorer),
    });
    Ok(rescorer)
}

fn process_event(handle: &mut SlimeHandle, event: InputEvent) -> Vec<SlimeAction> {
    #[cfg(feature = "neural")]
    let should_rerank = event == InputEvent::Space && handle.engine.phase() == Phase::Composing;
    #[cfg(feature = "neural")]
    let live_prior = (should_rerank
        && handle
            .neural
            .as_ref()
            .is_some_and(|runtime| runtime.explicit_live_agreement))
    .then(|| {
        handle
            .engine
            .current_live_neural_surface()
            .map(str::to_owned)
    })
    .flatten();
    let actions = handle.engine.handle(event);

    #[cfg(feature = "neural")]
    if should_rerank
        && let Some(runtime) = handle.neural.as_ref()
        && let Some(reranked_actions) =
            runtime.rank_engine(&mut handle.engine, live_prior.as_deref())
    {
        return reranked_actions;
    }

    actions
}

#[cfg(feature = "neural")]
struct ExplicitRecombinationScores {
    logliks: Vec<f64>,
    baseline: Vec<String>,
}

#[cfg(feature = "neural")]
impl NeuralRuntime {
    fn rank_engine(
        &self,
        engine: &mut SlimeEngine,
        live_prior: Option<&str>,
    ) -> Option<Vec<SlimeAction>> {
        if !self.explicit_recombination || !self.explicit_confidence {
            return engine.rank_candidates_with(|request| self.rank(request, live_prior));
        }
        let mut state = None;
        self.rescorer.with_interactive_scoring(|score| {
            engine.rank_candidates_with_recombination(|request| {
                self.recombination_plan(request, &mut state, score, live_prior)
            })
        })
    }

    fn recombination_plan(
        &self,
        request: &CandidateRankingRequest,
        state: &mut Option<ExplicitRecombinationScores>,
        score: &mut dyn FnMut(&ScoreRequest) -> Result<ScoredItem, String>,
        live_prior: Option<&str>,
    ) -> Option<slime_core::CandidateRankingPlan> {
        let Some(mut initial) = state.take() else {
            return self.initial_recombination_plan(request, state, score, live_prior);
        };
        let count = initial.logliks.len();
        let additions = request.candidates.get(count..)?;
        if additions.is_empty() || additions.len() > 2 {
            return None;
        }
        let scored = score(&ScoreRequest {
            context: request.left_context.clone(),
            input_katakana: hiragana_to_katakana(&request.reading),
            candidates: additions.iter().map(|c| c.surface.clone()).collect(),
        })
        .ok()?;
        if scored.logliks.len() != additions.len() {
            return None;
        }
        initial.logliks.extend(scored.logliks);
        initial
            .baseline
            .extend(additions.iter().map(|c| c.surface.clone()));
        let ranked = confident_explicit_ranking(request, &initial.logliks, &initial.baseline)?;
        // Expand the menu only when a newly generated candidate wins.
        if !additions.iter().any(|c| Some(&c.surface) == ranked.first()) {
            return None;
        }
        Some(slime_core::CandidateRankingPlan {
            ranked_surfaces: ranked,
            recombination_sources: None,
        })
    }

    fn initial_recombination_plan(
        &self,
        request: &CandidateRankingRequest,
        state: &mut Option<ExplicitRecombinationScores>,
        score: &mut dyn FnMut(&ScoreRequest) -> Result<ScoredItem, String>,
        live_prior: Option<&str>,
    ) -> Option<slime_core::CandidateRankingPlan> {
        if !should_score_neurally(request, self.explicit_max_cost_gap.or(self.max_cost_gap)) {
            return None;
        }
        let lambda = explicit_neural_lambda(
            self.lambda,
            self.explicit_long_reading_weight,
            self.explicit_medium_reading_weight,
            &request.reading,
        );
        let scored = score(&ScoreRequest {
            context: request.left_context.clone(),
            input_katakana: hiragana_to_katakana(&request.reading),
            candidates: request
                .candidates
                .iter()
                .map(|c| c.surface.clone())
                .collect(),
        })
        .ok()?;
        let stable = ranked_candidate_surfaces(
            request,
            &scored.logliks,
            lambda,
            0.0,
            SurfaceLengthPolicy::Unrestricted,
            None,
            None,
        )?;
        let eligible = lambda < 0.8 && request.reading.chars().count() >= 20;
        let confident = eligible
            .then(|| confident_explicit_ranking(request, &scored.logliks, &stable))
            .flatten();
        let sources = if eligible && confident.is_none() && request.left_context.is_empty() {
            explicit_confidence_ranking(request, &scored.logliks, &stable, false)
                .and_then(|ranked| Some([stable.first()?.clone(), ranked.first()?.clone()]))
        } else {
            None
        };
        let mut baseline = confident.unwrap_or(stable);
        if let Some(prior) = live_prior {
            apply_explicit_live_agreement(
                request,
                &scored.logliks,
                &scored.candidate_logliks,
                lambda,
                prior,
                &mut baseline,
            );
        }
        if !should_score_neurally(request, self.max_cost_gap)
            && !explicit_switch_preserves_hiragana(
                &request.candidates.first()?.surface,
                baseline.first()?,
            )
        {
            return None;
        }
        if sources.is_some() {
            *state = Some(ExplicitRecombinationScores {
                logliks: scored.logliks,
                baseline: baseline.clone(),
            });
        }
        Some(slime_core::CandidateRankingPlan {
            ranked_surfaces: baseline,
            recombination_sources: sources,
        })
    }

    fn rank(
        &self,
        request: &CandidateRankingRequest,
        live_prior: Option<&str>,
    ) -> Option<Vec<String>> {
        if !should_score_neurally(request, self.explicit_max_cost_gap.or(self.max_cost_gap)) {
            return None;
        }
        let lambda = explicit_neural_lambda(
            self.lambda,
            self.explicit_long_reading_weight,
            self.explicit_medium_reading_weight,
            &request.reading,
        );
        let scored = self
            .rescorer
            .score_interactive(&ScoreRequest {
                context: request.left_context.clone(),
                input_katakana: hiragana_to_katakana(&request.reading),
                candidates: request
                    .candidates
                    .iter()
                    .map(|c| c.surface.clone())
                    .collect(),
            })
            .ok()?;
        let baseline = ranked_candidate_surfaces(
            request,
            &scored.logliks,
            lambda,
            0.0,
            SurfaceLengthPolicy::Unrestricted,
            None,
            None,
        )?;
        let mut ranked =
            if self.explicit_confidence && lambda < 0.8 && request.reading.chars().count() >= 20 {
                confident_explicit_ranking(request, &scored.logliks, &baseline).unwrap_or(baseline)
            } else {
                baseline
            };
        if let Some(prior) = live_prior {
            apply_explicit_live_agreement(
                request,
                &scored.logliks,
                &scored.candidate_logliks,
                lambda,
                prior,
                &mut ranked,
            );
        }
        if !should_score_neurally(request, self.max_cost_gap)
            && !explicit_switch_preserves_hiragana(
                &request.candidates.first()?.surface,
                ranked.first()?,
            )
        {
            return None;
        }
        Some(ranked)
    }

    fn rank_live_with_lambda_fallback_after_gate(
        &self,
        snapshot: &LiveCandidateRankingSnapshot,
        request: &CandidateRankingRequest,
        lambda: f64,
        fallback_lambda: f64,
        policy: LiveRankingPolicy,
    ) -> Option<Vec<String>> {
        let score_request = ScoreRequest {
            context: request.left_context.clone(),
            input_katakana: hiragana_to_katakana(&request.reading),
            candidates: request
                .candidates
                .iter()
                .map(|candidate| candidate.surface.clone())
                .collect(),
        };
        let scored = self.rescorer.score_interactive(&score_request).ok()?;
        let rank = |lambda| {
            ranked_candidate_surfaces_with_current(
                request,
                &scored.logliks,
                lambda,
                policy.minimum_switch_margin,
                policy.surface_length,
                policy.numeric_base_switch_margin,
                policy.dictionary_base_switch_margin,
                policy.current_surface_index,
            )
        };
        let ranked = rank(lambda).or_else(|| {
            should_try_live_lambda_fallback(&request.reading, lambda, fallback_lambda)
                .then(|| rank(fallback_lambda))
                .flatten()
        });
        let ranked = ranked.map(|baseline| {
            body_supported_live_ranking(
                request,
                &scored.logliks,
                &scored.candidate_logliks,
                lambda,
                policy,
                &baseline,
            )
            .unwrap_or(baseline)
        });
        if snapshot.request_has_verb_auxiliary_candidates(request)
            && let Some(alternative) = ranked_candidate_surfaces_with_current(
                request,
                &scored.logliks,
                0.8,
                policy.minimum_switch_margin.max(1.0),
                SurfaceLengthPolicy::AllowWithMargin(1.0),
                policy.numeric_base_switch_margin,
                policy.dictionary_base_switch_margin,
                policy.current_surface_index,
            )
            && alternative.first().is_some_and(|selected| {
                snapshot.approve_model_supported_verb_auxiliary(
                    request,
                    selected,
                    &scored.logliks,
                    &scored.candidate_logliks,
                )
            })
        {
            return Some(alternative);
        }
        if let Some(selected) = ranked.as_ref().and_then(|surfaces| surfaces.first()) {
            snapshot.approve_model_supported_object_inflection(request, selected, &scored.logliks);
        }
        ranked
    }
}

// Reuse one decode, discount EOS for an alternate ranking, and require body
// likelihood support before replacing an ordinary LIVE winner.
#[cfg(any(feature = "neural", test))]
fn body_supported_live_ranking(
    request: &CandidateRankingRequest,
    full_scores: &[f64],
    body_scores: &[f64],
    lambda: f64,
    policy: LiveRankingPolicy,
    baseline: &[String],
) -> Option<Vec<String>> {
    if full_scores.len() != body_scores.len()
        || full_scores
            .iter()
            .chain(body_scores)
            .any(|score| !score.is_finite())
    {
        return None;
    }
    let scores: Vec<f64> = full_scores
        .iter()
        .zip(body_scores)
        .map(|(full, body)| 0.5 * full + 0.5 * body)
        .collect();
    if !(0.6..0.8).contains(&lambda)
        || request.reading.chars().count() < 4
        || scores.len() != request.candidates.len()
        || scores.iter().any(|score| !score.is_finite())
    {
        return None;
    }
    let margin = policy.minimum_switch_margin.max(0.5);
    let ranked = ranked_candidate_surfaces_with_current(
        request,
        &scores,
        0.8,
        margin,
        policy.surface_length,
        policy.numeric_base_switch_margin.map(|m| m.max(margin)),
        policy.dictionary_base_switch_margin.map(|m| m.max(margin)),
        policy.current_surface_index,
    )?;
    let before = baseline.first()?;
    let selected = ranked.first()?;
    if before == selected
        || !before
            .chars()
            .filter(|c| !is_han(*c))
            .eq(selected.chars().filter(|c| !is_han(*c)))
        || !explicit_changes_share_replacement(before, selected)
    {
        return None;
    }
    let before_index = request
        .candidates
        .iter()
        .position(|c| c.surface == *before)?;
    let selected_index = request
        .candidates
        .iter()
        .position(|c| c.surface == *selected)?;
    if body_scores[selected_index] - body_scores[before_index] < 0.5 {
        return None;
    }
    Some(ranked)
}

// A newly admitted full request must not suppress the existing suffix retry
// without improving the visible text. Apply this after all dictionary-tail
// postprocessing, which can otherwise split a katakana word into a particle.
#[cfg(any(feature = "neural", test))]
fn wider_live_winner_is_useful(current: &str, selected: &str, reopens_prefix: bool) -> bool {
    (!reopens_prefix || current != selected)
        && current
            .split(|character| !matches!(character, 'ァ'..='ヺ' | 'ー' | 'ｦ'..='ﾟ'))
            .filter(|run| !run.is_empty())
            .all(|run| selected.contains(run))
}

#[cfg(any(feature = "neural", test))]
fn confident_explicit_ranking(
    request: &CandidateRankingRequest,
    logliks: &[f64],
    baseline: &[String],
) -> Option<Vec<String>> {
    explicit_confidence_ranking(request, logliks, baseline, true)
}

// Retain an immediately preceding approved LIVE surface only for a close
// explicit decision supported by the same neural scores. No extra inference.
#[cfg(any(feature = "neural", test))]
fn apply_explicit_live_agreement(
    request: &CandidateRankingRequest,
    logliks: &[f64],
    body_logliks: &[f64],
    lambda: f64,
    prior: &str,
    ranked: &mut [String],
) {
    if !(8..20).contains(&request.reading.chars().count())
        || logliks.len() != request.candidates.len()
        || logliks.iter().any(|score| !score.is_finite())
    {
        return;
    }
    let Some(before) = ranked.first() else {
        return;
    };
    // Keep all literal text intact, including both kana scripts, Latin text,
    // punctuation and digits. This policy only resolves Han alternatives.
    let literal = |c: &char| !is_han(*c);
    if before == prior
        || !before
            .chars()
            .filter(literal)
            .eq(prior.chars().filter(literal))
    {
        return;
    }
    let Some(winner) = request.candidates.iter().position(|c| &c.surface == before) else {
        return;
    };
    let Some(previous) = request.candidates.iter().position(|c| c.surface == prior) else {
        return;
    };
    let score = |i: usize| {
        (1.0 - lambda) * (-f64::from(request.candidates[i].cost) / COST_LOG_SCALE)
            + lambda * logliks[i]
    };
    let model_delta = logliks[previous] - logliks[winner];
    // Retain a validated LIVE surface across a larger dictionary disagreement
    // only when both the full score and the candidate body strongly support it.
    // An EOS advantage alone must not expand the ordinary agreement window.
    let maximum_gap = if model_delta >= 2.0
        && body_logliks.len() == logliks.len()
        && body_logliks.iter().all(|score| score.is_finite())
        && body_logliks[previous] - body_logliks[winner] >= 2.0
        && explicit_changes_share_replacement(before, prior)
    {
        0.6
    } else {
        0.2
    };
    if model_delta <= 0.0 || score(winner) - score(previous) > maximum_gap {
        return;
    }
    if let Some(position) = ranked.iter().position(|surface| surface == prior) {
        ranked[..=position].rotate_right(1);
    }
}

#[cfg(any(feature = "neural", test))]
fn explicit_confidence_ranking(
    request: &CandidateRankingRequest,
    logliks: &[f64],
    baseline: &[String],
    require_consistent_replacement: bool,
) -> Option<Vec<String>> {
    if logliks.len() != request.candidates.len() || logliks.iter().any(|s| !s.is_finite()) {
        return None;
    }
    let ranked = ranked_candidate_surfaces(
        request,
        logliks,
        0.8,
        0.0,
        SurfaceLengthPolicy::Unrestricted,
        None,
        None,
    )?;
    let selected = ranked.first()?;
    let before = baseline.first()?;
    if selected == before
        || !explicit_switch_preserves_hiragana(before, selected)
        || (require_consistent_replacement && !explicit_changes_share_replacement(before, selected))
    {
        return None;
    }
    let digits = |c: &char| c.is_ascii_digit() || matches!(c, '０'..='９');
    if !before
        .chars()
        .filter(digits)
        .eq(selected.chars().filter(digits))
    {
        return None;
    }
    let winner = request
        .candidates
        .iter()
        .position(|c| &c.surface == selected)?;
    let score = |i: usize| {
        0.2 * (-f64::from(request.candidates[i].cost) / COST_LOG_SCALE) + 0.8 * logliks[i]
    };
    if request
        .candidates
        .iter()
        .enumerate()
        .any(|(i, _)| i != winner && score(winner) - score(i) < 1.0)
    {
        return None;
    }
    Some(ranked)
}

// Compare written runs separated by literal text, without allocating. This is
// a surface consistency guard, not dictionary word segmentation.
#[cfg(any(feature = "neural", test))]
fn explicit_changes_share_replacement(mut before: &str, mut after: &str) -> bool {
    let mut replacement = None;
    loop {
        let ((a_word, a), (b_word, b)) = match (
            take_confidence_run(&mut before),
            take_confidence_run(&mut after),
        ) {
            (Some(a), Some(b)) => (a, b),
            (None, None) => return replacement.is_some(),
            _ => return false,
        };
        if a_word != b_word {
            return false;
        }
        if a == b {
            continue;
        }
        if !a_word {
            return false;
        }
        let prefix = a
            .chars()
            .zip(b.chars())
            .take_while(|(x, y)| x == y)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let (a, b) = (&a[prefix..], &b[prefix..]);
        let suffix = a
            .chars()
            .rev()
            .zip(b.chars().rev())
            .take_while(|(x, y)| x == y)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let pair = (&a[..a.len() - suffix], &b[..b.len() - suffix]);
        if replacement.is_some_and(|previous| previous != pair) {
            return false;
        }
        replacement = Some(pair);
    }
}

#[cfg(any(feature = "neural", test))]
fn take_confidence_run<'a>(remaining: &mut &'a str) -> Option<(bool, &'a str)> {
    let text = *remaining;
    let word = |c: char| {
        is_han(c)
            || c.is_ascii_alphanumeric()
            || matches!(c, 'ァ'..='ヺ' | 'ー' | '々' | '０'..='９')
    };
    let kind = word(text.chars().next()?);
    let end = text
        .char_indices()
        .find(|(_, c)| word(*c) != kind)
        .map_or(text.len(), |(i, _)| i);
    *remaining = &text[end..];
    Some((kind, &text[..end]))
}

#[cfg(any(feature = "neural", test))]
fn explicit_switch_preserves_hiragana(base: &str, selected: &str) -> bool {
    base.chars()
        .filter(|c| matches!(c, '\u{3041}'..='\u{3096}'))
        .eq(selected
            .chars()
            .filter(|c| matches!(c, '\u{3041}'..='\u{3096}')))
}

#[cfg(any(feature = "neural", test))]
fn base_cost_gap(request: &CandidateRankingRequest) -> Option<i32> {
    let first = request.candidates.first()?.cost;
    request
        .candidates
        .iter()
        .skip(1)
        .map(|candidate| candidate.cost)
        .min()
        .map(|alternative| alternative.saturating_sub(first).max(0))
}

#[cfg(any(feature = "neural", test))]
fn should_score_neurally(request: &CandidateRankingRequest, max_cost_gap: Option<i32>) -> bool {
    max_cost_gap.is_none_or(|maximum| base_cost_gap(request).is_some_and(|gap| gap <= maximum))
}

#[cfg(any(feature = "neural", test))]
fn should_score_live_neurally(
    request: &CandidateRankingRequest,
    max_cost_gap: Option<i32>,
) -> bool {
    let live_max_cost_gap = max_cost_gap.map(|maximum| {
        let length = request.reading.chars().count();
        if length >= LIVE_RELAXED_COST_GAP_MIN_TARGET_CHARACTERS
            || (!request.left_context.is_empty()
                && length >= LIVE_CONTEXTUAL_COST_GAP_MIN_TARGET_CHARACTERS)
        {
            maximum.max(LIVE_LONG_TARGET_MAX_COST_GAP)
        } else {
            maximum
        }
    });
    should_score_neurally(request, live_max_cost_gap)
}

#[cfg(feature = "neural")]
fn should_confirm_live_contextual_base(
    snapshot: &LiveCandidateRankingSnapshot,
    request: &CandidateRankingRequest,
) -> bool {
    if request.left_context.is_empty()
        || !(4..=8).contains(&request.reading.chars().count())
        || snapshot.request_reopens_stable_prefix(request)
        || base_cost_gap(request).is_none_or(|gap| gap > 2_500)
    {
        return false;
    }
    request.candidates.first().is_some_and(|candidate| {
        snapshot
            .base_target_surface_for_request(request)
            .is_some_and(|target| {
                target != candidate.surface
                    && target.chars().count() == 2
                    && target.chars().all(is_han)
                    && candidate.surface.chars().count() == 2
                    && candidate.surface.chars().all(is_han)
            })
    })
}

#[cfg(any(feature = "neural", test))]
fn base_ranked_candidate_surfaces(request: &CandidateRankingRequest) -> Vec<String> {
    request
        .candidates
        .iter()
        .map(|candidate| candidate.surface.clone())
        .collect()
}

#[cfg(any(feature = "neural", test))]
fn explicit_neural_lambda(
    base_lambda: f64,
    long_reading_weight: Option<(usize, f64)>,
    medium_reading_weight: Option<f64>,
    reading: &str,
) -> f64 {
    let length = reading.chars().count();
    long_reading_weight
        .filter(|(minimum, _)| length >= *minimum)
        .map(|(_, lambda)| lambda)
        .or_else(|| medium_reading_weight.filter(|_| (3..20).contains(&length)))
        .unwrap_or(base_lambda)
}

#[cfg(any(feature = "neural", test))]
fn live_neural_lambda(base_lambda: f64, long_reading_lambda: f64, reading: &str) -> f64 {
    if reading.chars().count() >= LIVE_LONG_READING_MIN_CHARACTERS {
        long_reading_lambda
    } else {
        base_lambda
    }
}

#[cfg(any(feature = "neural", test))]
fn live_neural_switch_margin(base_margin: f64, long_reading_margin: f64, reading: &str) -> f64 {
    if reading.chars().count() >= LIVE_LONG_READING_MIN_CHARACTERS {
        long_reading_margin
    } else {
        base_margin
    }
}

#[cfg(any(feature = "neural", test))]
fn should_try_live_lambda_fallback(reading: &str, lambda: f64, fallback_lambda: f64) -> bool {
    reading.chars().count() >= LIVE_LAMBDA_FALLBACK_MIN_TARGET_CHARACTERS
        && (lambda - fallback_lambda).abs() > f64::EPSILON
}

#[cfg(feature = "neural")]
fn live_surface_length_policy(
    snapshot: &LiveCandidateRankingSnapshot,
    request: &CandidateRankingRequest,
) -> SurfaceLengthPolicy {
    let length = request.reading.chars().count();
    let allows_long_target_change = (length >= LIVE_LENGTH_CHANGE_MIN_TARGET_CHARACTERS
        || (length >= LIVE_CONTEXTUAL_LENGTH_CHANGE_MIN_TARGET_CHARACTERS
            && !request.left_context.is_empty()))
        && !snapshot.request_reopens_stable_prefix(request);
    if allows_long_target_change || snapshot.request_has_bounded_kanji_run_length_repair(request) {
        SurfaceLengthPolicy::AllowWithMargin(LIVE_LENGTH_CHANGE_SWITCH_MARGIN)
    } else {
        SurfaceLengthPolicy::Preserve
    }
}

#[cfg(any(feature = "neural", test))]
fn candidate_family_is_name_spelling_ambiguity(request: &CandidateRankingRequest) -> bool {
    if request.left_context.is_empty()
        || !(4..=8).contains(&request.reading.chars().count())
        || request.candidates.len() < 4
    {
        return false;
    }
    let leading: Vec<Vec<char>> = request
        .candidates
        .iter()
        .take(4)
        .map(|candidate| candidate.surface.chars().collect())
        .collect();
    let width = leading[0].len();
    if !(2..=4).contains(&width)
        || leading
            .iter()
            .any(|surface| surface.len() != width || !surface.iter().all(|char| is_han(*char)))
    {
        return false;
    }
    leading
        .iter()
        .all(|surface| surface.first() == leading[0].first())
        || leading
            .iter()
            .all(|surface| surface.last() == leading[0].last())
}

#[cfg(any(feature = "neural", test))]
fn is_han(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
    )
}

#[cfg(any(feature = "neural", test))]
fn ranked_candidate_surfaces(
    request: &CandidateRankingRequest,
    logliks: &[f64],
    lambda: f64,
    minimum_switch_margin: f64,
    surface_length_policy: SurfaceLengthPolicy,
    numeric_base_switch_margin: Option<f64>,
    dictionary_base_switch_margin: Option<f64>,
) -> Option<Vec<String>> {
    ranked_candidate_surfaces_with_current(
        request,
        logliks,
        lambda,
        minimum_switch_margin,
        surface_length_policy,
        numeric_base_switch_margin,
        dictionary_base_switch_margin,
        None,
    )
}

#[cfg(any(feature = "neural", test))]
#[allow(clippy::too_many_arguments)]
fn ranked_candidate_surfaces_with_current(
    request: &CandidateRankingRequest,
    logliks: &[f64],
    lambda: f64,
    minimum_switch_margin: f64,
    surface_length_policy: SurfaceLengthPolicy,
    numeric_base_switch_margin: Option<f64>,
    dictionary_base_switch_margin: Option<f64>,
    current_surface_index: Option<usize>,
) -> Option<Vec<String>> {
    if request.candidates.len() != logliks.len() {
        return None;
    }
    let mut indexed: Vec<_> = (0..request.candidates.len()).collect();
    let combined: Vec<_> = request
        .candidates
        .iter()
        .zip(logliks)
        .map(|(candidate, loglik)| {
            (1.0 - lambda) * (-f64::from(candidate.cost) / COST_LOG_SCALE) + lambda * loglik
        })
        .collect();
    indexed.sort_by(|&left, &right| combined[right].total_cmp(&combined[left]));
    let mut used_length_fallback = false;
    if surface_length_policy == SurfaceLengthPolicy::Preserve {
        let original_winner = *indexed.first()?;
        let original_runner_up = *indexed.get(1)?;
        let base = request.candidates.first()?;
        let width = base.surface.chars().count();
        let winner_surface = &request.candidates[original_winner].surface;
        let numeric_repair = numeric_base_switch_margin.is_some()
            && !contains_numeric_character(&request.reading)
            && contains_numeric_character(&base.surface)
            && !contains_numeric_character(winner_surface);
        if !numeric_repair && winner_surface.chars().count() != width {
            if combined[original_winner] - combined[original_runner_up] < minimum_switch_margin {
                return None;
            }
            indexed.retain(|&index| request.candidates[index].surface.chars().count() == width);
            used_length_fallback = true;
        }
    }
    let winner = *indexed.first()?;
    let runner_up = *indexed.get(1)?;
    if used_length_fallback {
        let current = current_surface_index.and_then(|index| combined.get(index))?;
        if combined[winner] - current < minimum_switch_margin {
            return None;
        }
    }

    let numeric_base_repair = numeric_base_switch_margin.is_some()
        && !contains_numeric_character(&request.reading)
        && contains_numeric_character(&request.candidates[0].surface)
        && !contains_numeric_character(&request.candidates[winner].surface);
    let dictionary_base_repair = winner == 0 && dictionary_base_switch_margin.is_some();
    let required_margin = if numeric_base_repair {
        minimum_switch_margin.min(numeric_base_switch_margin.unwrap_or(f64::INFINITY))
    } else if dictionary_base_repair {
        minimum_switch_margin.min(dictionary_base_switch_margin.unwrap_or(f64::INFINITY))
    } else {
        minimum_switch_margin
    };
    if combined[winner] - combined[runner_up] < required_margin {
        return None;
    }
    if winner != 0 {
        let base = &request.candidates[0].surface;
        let winner_surface = &request.candidates[winner].surface;
        let switches_surface_length =
            !numeric_base_repair && winner_surface.chars().count() != base.chars().count();
        let required_base_margin = match surface_length_policy {
            SurfaceLengthPolicy::Preserve if switches_surface_length => return None,
            SurfaceLengthPolicy::AllowWithMargin(margin) if switches_surface_length => {
                required_margin.max(margin)
            }
            SurfaceLengthPolicy::Unrestricted
            | SurfaceLengthPolicy::Preserve
            | SurfaceLengthPolicy::AllowWithMargin(_) => required_margin,
        };
        if !numeric_base_repair
            && request.candidates[winner]
                .cost
                .saturating_sub(request.candidates[0].cost)
                >= LIVE_DEEP_TAIL_SWITCH_MIN_COST_GAP
            && rewrites_only_short_surface_tail(base, winner_surface)
        {
            // A language-model preference must not resurrect a very deep
            // dictionary homophone by changing only the unfinished two-char
            // tail. These paths are overwhelmingly lexical lookalikes such as
            // an inflected verb stem becoming an unrelated noun; the already
            // context-ranked base is the safer LIVE display.
            return None;
        }
        if !numeric_base_repair
            && request.candidates[winner]
                .cost
                .saturating_sub(request.candidates[0].cost)
                >= LIVE_DEEP_COMPOUND_SWITCH_MIN_COST_GAP
            && rewrites_one_han_inside_compound(base, winner_surface)
        {
            // Small language models can strongly prefer a familiar but wrong
            // compound spelling. Do not let a single in-run Han substitution
            // resurrect a substantially deeper path; close alternatives stay
            // rankable and inflected one-Han stems are outside this shape.
            return None;
        }
        if combined[winner] - combined[0] < required_base_margin {
            return None;
        }
    }
    if used_length_fallback {
        restore_excluded_candidate_indexes(&mut indexed, request.candidates.len());
    }
    Some(
        indexed
            .into_iter()
            .map(|index| request.candidates[index].surface.clone())
            .collect(),
    )
}

#[cfg(any(feature = "neural", test))]
fn restore_excluded_candidate_indexes(indexed: &mut Vec<usize>, candidate_count: usize) {
    // Application requires a complete permutation, including candidates that
    // were ineligible to win under the length policy.
    for index in 0..candidate_count {
        if !indexed.contains(&index) {
            indexed.push(index);
        }
    }
}

#[cfg(any(feature = "neural", test))]
fn rewrites_only_short_surface_tail(base: &str, selected: &str) -> bool {
    let base_characters = base.chars().count();
    base_characters == selected.chars().count()
        && base
            .chars()
            .zip(selected.chars())
            .take_while(|(base, selected)| base == selected)
            .count()
            >= base_characters.saturating_sub(LIVE_DEEP_TAIL_SWITCH_MAX_CHARACTERS)
}

#[cfg(any(feature = "neural", test))]
fn rewrites_one_han_inside_compound(base: &str, selected: &str) -> bool {
    let base: Vec<_> = base.chars().collect();
    let selected: Vec<_> = selected.chars().collect();
    if base.len() != selected.len() {
        return false;
    }
    let mut differences = base
        .iter()
        .zip(&selected)
        .enumerate()
        .filter(|(_, (base, selected))| base != selected);
    let Some((index, (base_character, selected_character))) = differences.next() else {
        return false;
    };
    if differences.next().is_some() || !is_han(*base_character) || !is_han(*selected_character) {
        return false;
    }
    let is_inside_han_run = |surface: &[char]| {
        index
            .checked_sub(1)
            .is_some_and(|left| is_han(surface[left]))
            && surface.get(index + 1).is_some_and(|right| is_han(*right))
    };
    is_inside_han_run(&base) && is_inside_han_run(&selected)
}

#[cfg(any(feature = "neural", test))]
fn contains_numeric_character(surface: &str) -> bool {
    surface
        .chars()
        .any(|character| character.is_ascii_digit() || matches!(character, '０'..='９'))
}

#[cfg(any(feature = "neural", test))]
fn dictionary_base_introduces_implicit_numeric(request: &CandidateRankingRequest) -> bool {
    !contains_numeric_character(&request.reading)
        && request
            .candidates
            .first()
            .is_some_and(|candidate| contains_numeric_character(&candidate.surface))
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

/// Breaks transient left context after an external caret, document, or input
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
            selected: NO_SELECTION,
            selection_start: NO_SELECTION,
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
            selected: NO_SELECTION,
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
                selection_start: NO_SELECTION,
                selection_length: 0,
            }
        }
        SlimeAction::HideCandidates => action_without_payload(ACTION_HIDE_CANDIDATES),
        SlimeAction::Commit(text) => SlimeActionView {
            kind: ACTION_COMMIT,
            text: SlimeStringView::new(text),
            candidates: ptr::null(),
            candidate_count: 0,
            selected: NO_SELECTION,
            selection_start: NO_SELECTION,
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
            selected: NO_SELECTION,
            selection_start: NO_SELECTION,
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
            selected: NO_SELECTION,
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
                selection_start: NO_SELECTION,
                selection_length: 0,
            }
        }
        SlimeAction::HideCandidates => action_without_payload_v2(ACTION_HIDE_CANDIDATES),
        SlimeAction::Commit(text) => SlimeActionViewV2 {
            kind: ACTION_COMMIT,
            text: SlimeStringView::new(text),
            candidates: ptr::null(),
            candidate_count: 0,
            selected: NO_SELECTION,
            selection_start: NO_SELECTION,
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
        selected: NO_SELECTION,
        selection_start: NO_SELECTION,
        selection_length: 0,
    }
}

const fn action_without_payload_v2(kind: u32) -> SlimeActionViewV2 {
    SlimeActionViewV2 {
        kind,
        text: SlimeStringView::empty(),
        candidates: ptr::null(),
        candidate_count: 0,
        selected: NO_SELECTION,
        selection_start: NO_SELECTION,
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
    use super::{
        ACTION_COMMIT, ACTION_SHOW_CANDIDATES, ACTION_UPDATE_PREEDIT,
        CANDIDATE_ANNOTATION_CORRECTION, CANDIDATE_ANNOTATION_NUMBER, EVENT_ACCEPT_CANDIDATE,
        EVENT_CHARACTER, EVENT_ENTER, EVENT_NEXT_CANDIDATE, EVENT_PREVIOUS_SEGMENT,
        EVENT_SELECT_CANDIDATE, EVENT_SPACE, LIVE_RELAXED_COST_GAP_MIN_TARGET_CHARACTERS,
        STATUS_INVALID_CANDIDATE, STATUS_INVALID_COST_GAP, STATUS_INVALID_UTF8,
        STATUS_INVALID_WEIGHT, STATUS_NULL_HANDLE, STATUS_OK, SlimeActionView, SlimeActionViewV2,
        SlimeBuffer, SlimeStringView, SurfaceLengthPolicy, base_cost_gap,
        base_ranked_candidate_surfaces, candidate_family_is_name_spelling_ambiguity,
        dictionary_base_introduces_implicit_numeric, live_neural_lambda, live_neural_switch_margin,
        ranked_candidate_surfaces, ranked_candidate_surfaces_with_current,
        rewrites_one_han_inside_compound, should_score_live_neurally, should_score_neurally,
        should_try_live_lambda_fallback, should_use_dictionary_base_after_rejected_neural,
        slime_begin_reconversion, slime_buffer_destroy, slime_conversion_candidates, slime_create,
        slime_create_with_data_dir, slime_create_with_signed_data_dir,
        slime_create_with_signed_data_dir_and_version_floors, slime_destroy,
        slime_domain_dictionary_words, slime_enable_neural_reranker,
        slime_enable_neural_reranker_with_cost_gap, slime_installed_dictionary_pack_words,
        slime_installed_dictionary_packs, slime_process, slime_process_actions,
        slime_process_actions_v2, slime_record_external_selection, slime_reset_context,
        slime_set_external_left_context, slime_set_options, slime_set_options_v2,
        slime_set_options_v3, slime_set_options_v4, slime_set_options_v5,
    };
    #[cfg(not(feature = "neural"))]
    use super::{
        STATUS_NEURAL_UNAVAILABLE, slime_live_neural_task_create, slime_live_neural_task_destroy,
        slime_live_neural_task_run, slime_set_live_neural_ranking_enabled,
    };
    use slime_core::{CandidateRankingItem, CandidateRankingRequest};
    use std::ffi::c_void;
    use std::fs;

    #[test]
    fn live_body_support_keeps_the_permutation_and_existing_margin_limits() {
        let request = CandidateRankingRequest {
            reading: "じきそうりだいじんこうほ".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "時期総理大臣候補".to_owned(),
                    cost: 0,
                },
                CandidateRankingItem {
                    surface: "次期総理大臣候補".to_owned(),
                    cost: 1_000,
                },
            ],
        };
        let baseline: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        let policy = super::LiveRankingPolicy {
            minimum_switch_margin: 0.3,
            surface_length: SurfaceLengthPolicy::Preserve,
            numeric_base_switch_margin: None,
            dictionary_base_switch_margin: None,
            current_surface_index: None,
        };
        assert_eq!(
            super::body_supported_live_ranking(
                &request,
                &[-3.0, -1.5],
                &[-3.0, -1.5],
                0.6,
                policy,
                &baseline
            )
            .unwrap(),
            vec![baseline[1].clone(), baseline[0].clone()]
        );
        // A large EOS advantage must not override contrary or weak body evidence.
        for body in [
            vec![-1.0, -2.0],
            vec![-2.0, -1.6],
            vec![-2.0],
            vec![-2.0, f64::NAN],
        ] {
            assert!(
                super::body_supported_live_ranking(
                    &request,
                    &[-20.0, -2.0],
                    &body,
                    0.6,
                    policy,
                    &baseline,
                )
                .is_none()
            );
        }
        for lambda in [0.2, 0.5, 0.8, 1.0] {
            assert!(
                super::body_supported_live_ranking(
                    &request,
                    &[-3.0, -1.5],
                    &[-3.0, -1.5],
                    lambda,
                    policy,
                    &baseline
                )
                .is_none()
            );
        }
        for scores in [vec![-3.0], vec![-3.0, f64::NAN], vec![-3.0, -2.0]] {
            assert!(
                super::body_supported_live_ranking(
                    &request, &scores, &scores, 0.6, policy, &baseline
                )
                .is_none()
            );
        }
        assert!(
            super::body_supported_live_ranking(
                &request,
                &[-3.0, -1.5],
                &[-3.0, -1.5],
                0.6,
                super::LiveRankingPolicy {
                    minimum_switch_margin: 1.0,
                    ..policy
                },
                &baseline
            )
            .is_none()
        );
    }

    #[test]
    fn live_body_support_preserves_literals_and_rejects_independent_replacements() {
        let policy = super::LiveRankingPolicy {
            minimum_switch_margin: 0.3,
            surface_length: SurfaceLengthPolicy::Unrestricted,
            numeric_base_switch_margin: None,
            dictionary_base_switch_margin: None,
            current_surface_index: None,
        };
        for (before, after) in [
            ("資料2枚", "資料3枚"),
            ("資料を取る", "資料が取る"),
            ("本文をカク", "本文を書く"),
            ("政策と保障", "制作と保証"),
        ] {
            let request = CandidateRankingRequest {
                reading: "じゅうぶんなながさ".to_owned(),
                left_context: String::new(),
                candidates: vec![
                    CandidateRankingItem {
                        surface: before.to_owned(),
                        cost: 0,
                    },
                    CandidateRankingItem {
                        surface: after.to_owned(),
                        cost: 1_000,
                    },
                ],
            };
            assert!(
                super::body_supported_live_ranking(
                    &request,
                    &[-3.0, -1.5],
                    &[-3.0, -1.5],
                    0.6,
                    policy,
                    &[before.to_owned(), after.to_owned()]
                )
                .is_none()
            );
        }
    }

    #[test]
    fn wider_live_gate_preserves_suffix_retry_for_unchanged_full_requests() {
        assert!(!super::wider_live_winner_is_useful(
            "伝説とかしている",
            "伝説とかしている",
            true
        ));
        assert!(super::wider_live_winner_is_useful(
            "伝説とかしている",
            "伝説と化している",
            true
        ));
        assert!(super::wider_live_winner_is_useful(
            "化している",
            "化している",
            false
        ));
    }

    #[test]
    fn wider_live_gate_rejects_fragmented_katakana_after_postprocessing() {
        for reopens in [false, true] {
            assert!(!super::wider_live_winner_is_useful(
                "本革シート",
                "本革シーと",
                reopens
            ));
            assert!(!super::wider_live_winner_is_useful(
                "本革ｼｰﾄ",
                "本革ｼｰと",
                reopens
            ));
            assert!(super::wider_live_winner_is_useful(
                "本皮シート",
                "本革シート",
                reopens
            ));
            assert!(super::wider_live_winner_is_useful(
                "でぃーゼル",
                "ディーゼル",
                reopens
            ));
            assert!(super::wider_live_winner_is_useful(
                "いんど料理",
                "インド料理",
                reopens
            ));
        }
    }

    #[test]
    fn explicit_live_agreement_requires_close_scores_and_preserves_the_permutation() {
        let request = slime_core::CandidateRankingRequest {
            reading: "しりょうをかくにんする".to_owned(),
            left_context: String::new(),
            candidates: vec![
                slime_core::CandidateRankingItem {
                    surface: "資料を確認する".to_owned(),
                    cost: 0,
                },
                slime_core::CandidateRankingItem {
                    surface: "史料を確認する".to_owned(),
                    cost: 100,
                },
            ],
        };
        let original: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        let mut ranked = original.clone();
        super::apply_explicit_live_agreement(
            &request,
            &[0.0, 0.1],
            &[0.0, 0.1],
            0.3,
            &original[1],
            &mut ranked,
        );
        assert_eq!(ranked, vec![original[1].clone(), original[0].clone()]);
        for scores in [vec![0.0, -0.1], vec![0.0], vec![0.0, f64::NAN]] {
            let mut ranked = original.clone();
            super::apply_explicit_live_agreement(
                &request,
                &scores,
                &scores,
                0.3,
                &original[1],
                &mut ranked,
            );
            assert_eq!(ranked, original);
        }
        for (reading, prior, cost) in [
            ("しゅしん", "史料を確認する", 100),
            ("しりょうをかくにんする", "史料を確認する", 1000),
            ("しりょうをかくにんする", "候補外", 100),
            ("しりょうをかくにんする", "史料を確認した", 100),
            ("しりょうをかくにんする", "史料2を確認する", 100),
            ("しりょうをかくにんする", "史料カを確認する", 100),
            ("しりょうをかくにんする", "史料Aを確認する", 100),
            ("しりょうをかくにんする", "史料、を確認する", 100),
        ] {
            let mut changed = request.clone();
            changed.reading = reading.to_owned();
            changed.candidates[1].cost = cost;
            if prior != "候補外" {
                changed.candidates[1].surface = prior.to_owned();
            }
            let mut ranked: Vec<_> = changed
                .candidates
                .iter()
                .map(|c| c.surface.clone())
                .collect();
            let before = ranked.clone();
            super::apply_explicit_live_agreement(
                &changed,
                &[0.0, 0.1],
                &[0.0, 0.1],
                0.3,
                prior,
                &mut ranked,
            );
            assert_eq!(ranked, before);
        }
    }

    #[test]
    fn strong_live_agreement_needs_model_support_and_bounded_dictionary_disagreement() {
        let mut request = CandidateRankingRequest {
            reading: "しりょうをかくにんする".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "資料を確認する".to_owned(),
                    cost: 0,
                },
                CandidateRankingItem {
                    surface: "史料を確認する".to_owned(),
                    cost: 800,
                },
            ],
        };
        let original: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        for (delta, cost, switches) in [(2.0, 800, true), (1.9, 800, false), (2.0, 1000, false)] {
            request.candidates[1].cost = cost;
            let mut ranked = original.clone();
            super::apply_explicit_live_agreement(
                &request,
                &[-5.0, -5.0 + delta],
                &[-5.0, -5.0 + delta],
                0.3,
                &original[1],
                &mut ranked,
            );
            assert_eq!(ranked[0] == original[1], switches);
            ranked.sort();
            let mut expected = original.clone();
            expected.sort();
            assert_eq!(ranked, expected);
        }
        request.candidates[1].cost = 800;
        request.candidates[0].surface = "政策と保障".to_owned();
        request.candidates[1].surface = "制作と保証".to_owned();
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        let before = ranked.clone();
        super::apply_explicit_live_agreement(
            &request,
            &[-5.0, -3.0],
            &[-5.0, -3.0],
            0.3,
            &before[1],
            &mut ranked,
        );
        assert_eq!(ranked, before);
    }

    #[test]
    fn expanded_live_agreement_requires_body_support_but_preserves_ordinary_window() {
        let mut request = CandidateRankingRequest {
            reading: "しりょうをかくにんする".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "資料を確認する".to_owned(),
                    cost: 0,
                },
                CandidateRankingItem {
                    surface: "史料を確認する".to_owned(),
                    cost: 800,
                },
            ],
        };
        let original: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        let full = [-5.0, -3.0];
        for (body, expanded) in [
            (vec![-5.0, -3.0], true),
            (vec![-5.0, -3.01], false),
            (vec![-5.0, -7.0], false),
            (vec![], false),
            (vec![-5.0], false),
            (vec![-5.0, f64::NAN], false),
            (vec![f64::NEG_INFINITY, -3.0], false),
        ] {
            request.candidates[1].cost = 800;
            let mut ranked = original.clone();
            super::apply_explicit_live_agreement(
                &request,
                &full,
                &body,
                0.3,
                &original[1],
                &mut ranked,
            );
            assert_eq!(ranked[0] == original[1], expanded, "body: {body:?}");
            let mut sorted = ranked;
            sorted.sort();
            let mut expected = original.clone();
            expected.sort();
            assert_eq!(sorted, expected);
            // A close full-score decision retains the pre-existing agreement behavior,
            // even if body scores cannot support an expanded window.
            request.candidates[1].cost = 500;
            let mut ranked = original.clone();
            super::apply_explicit_live_agreement(
                &request,
                &full,
                &body,
                0.3,
                &original[1],
                &mut ranked,
            );
            assert_eq!(ranked[0], original[1]);
        }
    }

    #[test]
    fn neural_interpolation_preserves_base_order_or_uses_model_scores() {
        let request = CandidateRankingRequest {
            reading: "てすと".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "基底一位".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "モデル一位".to_owned(),
                    cost: 200,
                },
            ],
        };

        assert_eq!(
            ranked_candidate_surfaces(
                &request,
                &[-10.0, -1.0],
                0.0,
                0.0,
                SurfaceLengthPolicy::Unrestricted,
                None,
                None,
            )
            .unwrap(),
            ["基底一位", "モデル一位"]
        );
        assert_eq!(
            ranked_candidate_surfaces(
                &request,
                &[-10.0, -1.0],
                1.0,
                0.0,
                SurfaceLengthPolicy::Unrestricted,
                None,
                None,
            )
            .unwrap(),
            ["モデル一位", "基底一位"]
        );
        assert!(
            ranked_candidate_surfaces(
                &request,
                &[-10.0, -1.0],
                0.05,
                0.5,
                SurfaceLengthPolicy::Unrestricted,
                None,
                None,
            )
            .is_none(),
            "a weak model switch must not produce an automatic LIVE decision"
        );
        assert!(
            ranked_candidate_surfaces(
                &request,
                &[0.0],
                0.2,
                0.0,
                SurfaceLengthPolicy::Unrestricted,
                None,
                None,
            )
            .is_none()
        );
    }

    fn ranking_request(reading: &str, candidates: &[(&str, i32)]) -> CandidateRankingRequest {
        CandidateRankingRequest {
            reading: reading.to_owned(),
            left_context: String::new(),
            candidates: candidates
                .iter()
                .map(|&(surface, cost)| CandidateRankingItem {
                    surface: surface.to_owned(),
                    cost,
                })
                .collect(),
        }
    }

    #[test]
    fn length_fallback_keeps_all_candidates_when_it_improves_the_display() {
        let request = ranking_request(
            "ははとこども",
            &[
                ("母と子ども", 10655),
                ("母と子供", 10687),
                ("母とこども", 11595),
                ("は鳩子ども", 12506),
                ("は鳩子供", 12538),
                ("は葉と子ども", 12632),
                ("は葉と子供", 12664),
                ("ハハと子ども", 12677),
                ("ハハと子供", 12709),
                ("は歯と子ども", 12890),
            ],
        );
        let ranked = ranked_candidate_surfaces_with_current(
            &request,
            &[
                -26.206, -24.231, -30.449, -34.522, -33.044, -35.318, -32.809, -34.896, -32.031,
                -33.948,
            ],
            0.6,
            0.3,
            SurfaceLengthPolicy::Preserve,
            None,
            None,
            Some(2),
        )
        .unwrap();
        assert_eq!(ranked[0], "母と子ども");
        assert_eq!(ranked.len(), request.candidates.len());
        for candidate in &request.candidates {
            assert_eq!(
                ranked
                    .iter()
                    .filter(|surface| **surface == candidate.surface)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn length_fallback_rejects_a_ranking_that_does_not_improve_the_display() {
        let request = ranking_request(
            "ゆうじんとこや",
            &[
                ("友人床や", 11919),
                ("友人床屋", 12009),
                ("友人と小屋", 12205),
                ("友人とこや", 12403),
                ("友人トコや", 13381),
                ("有人床や", 13692),
                ("友人と子や", 13705),
                ("友人と娘や", 13757),
                ("有人床屋", 13782),
                ("ゆうじんとこや", 12419),
            ],
        );
        let ranked = ranked_candidate_surfaces_with_current(
            &request,
            &[
                -31.916, -27.189, -24.955, -26.267, -32.271, -30.325, -27.930, -37.971, -28.254,
                -27.322,
            ],
            0.6,
            0.3,
            SurfaceLengthPolicy::Preserve,
            None,
            None,
            Some(3),
        );
        assert!(ranked.is_none());
    }

    #[test]
    fn live_neural_ranking_preserves_the_base_surface_length() {
        let request = CandidateRankingRequest {
            reading: "あたり".to_owned(),
            left_context: "府中や国立の".to_owned(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "辺り".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "辺".to_owned(),
                    cost: 200,
                },
                CandidateRankingItem {
                    surface: "当たり".to_owned(),
                    cost: 300,
                },
            ],
        };

        assert!(
            ranked_candidate_surfaces(
                &request,
                &[-10.0, 0.0, -5.0],
                1.0,
                0.0,
                SurfaceLengthPolicy::Preserve,
                None,
                None,
            )
            .is_none()
        );
        assert_eq!(
            ranked_candidate_surfaces(
                &request,
                &[-10.0, 0.0, -5.0],
                1.0,
                0.0,
                SurfaceLengthPolicy::Unrestricted,
                None,
                None,
            )
            .unwrap()[0],
            "辺"
        );
        assert!(
            ranked_candidate_surfaces(
                &request,
                &[-0.4, 0.0, -5.0],
                1.0,
                0.0,
                SurfaceLengthPolicy::AllowWithMargin(0.5),
                None,
                None,
            )
            .is_none(),
            "a weak length-changing switch must remain blocked"
        );
        assert_eq!(
            ranked_candidate_surfaces(
                &request,
                &[-0.6, 0.0, -5.0],
                1.0,
                0.0,
                SurfaceLengthPolicy::AllowWithMargin(0.5),
                None,
                None,
            )
            .unwrap()[0],
            "辺",
            "a strong length-changing switch can be admitted by policy"
        );
    }

    #[test]
    fn explicit_live_agreement_configuration_rejects_unavailable_handles() {
        assert_eq!(
            unsafe { super::slime_set_explicit_live_agreement(std::ptr::null_mut(), true) },
            super::STATUS_NULL_HANDLE
        );
        let handle = slime_create();
        assert!(!handle.is_null());
        assert_eq!(
            unsafe { super::slime_set_explicit_live_agreement(handle, true) },
            super::STATUS_NEURAL_UNAVAILABLE
        );
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn explicit_confidence_configuration_rejects_unavailable_handles() {
        assert_eq!(
            unsafe { super::slime_set_explicit_neural_confidence(std::ptr::null_mut(), true) },
            super::STATUS_NULL_HANDLE
        );
        let handle = slime_create();
        assert!(!handle.is_null());
        assert_eq!(
            unsafe { super::slime_set_explicit_neural_confidence(handle, true) },
            super::STATUS_NEURAL_UNAVAILABLE
        );
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn recombination_relaxes_only_the_consistent_replacement_guard() {
        let before = "脳圧更新状態が続き機能が障害され";
        for (after, allowed) in [
            ("脳圧亢進状態が続き機能が傷害され", true),
            ("脳圧亢進状態を続き機能が傷害され", false),
            ("脳圧亢進状態が続き機能が傷害され2", false),
        ] {
            let request = CandidateRankingRequest {
                reading: "のうあつこうしんじょうたいがつづききのうがしょうがいされ".to_owned(),
                left_context: String::new(),
                candidates: [before, after]
                    .into_iter()
                    .map(|surface| slime_core::CandidateRankingItem {
                        surface: surface.to_owned(),
                        cost: 0,
                    })
                    .collect(),
            };
            let baseline = vec![before.to_owned(), after.to_owned()];
            assert!(super::confident_explicit_ranking(&request, &[-5.0, 0.0], &baseline).is_none());
            assert_eq!(
                super::explicit_confidence_ranking(&request, &[-5.0, 0.0], &baseline, false)
                    .is_some(),
                allowed
            );
            assert!(
                super::explicit_confidence_ranking(&request, &[-0.5, 0.0], &baseline, false)
                    .is_none()
            );
        }
    }

    #[test]
    fn explicit_confidence_repeats_one_surface_replacement_only() {
        for (before, after) in [
            (
                "筑波大学付属中学校、筑波大学付属高等学校",
                "筑波大学附属中学校、筑波大学附属高等学校",
            ),
            ("重症を負う", "重傷を負う"),
            ("エイト金", "エイトキン"),
        ] {
            assert!(super::explicit_changes_share_replacement(before, after));
        }
        for (before, after) in [
            (
                "脳圧更新状態が続き機能が障害され",
                "脳圧亢進状態が続き機能が傷害され",
            ),
            ("施策されたが、正式採用", "試作されたが、制式採用"),
            ("烏龍は標準通過", "ウーロンは標準通貨"),
            ("重症を負う", "重傷が負う"),
            ("同じ", "同じ"),
            ("重症", "重傷を"),
        ] {
            assert!(
                !super::explicit_changes_share_replacement(before, after),
                "{before} -> {after}"
            );
        }
    }

    #[test]
    fn explicit_confidence_requires_separation_from_every_alternative() {
        let request = CandidateRankingRequest {
            reading: "じゅうしょうをおう".to_owned(),
            left_context: String::new(),
            candidates: ["重症を負う", "重傷を負う", "重傷を追う"]
                .into_iter()
                .map(|surface| CandidateRankingItem {
                    surface: surface.to_owned(),
                    cost: 100,
                })
                .collect(),
        };
        let baseline = vec!["重症を負う".to_owned()];
        assert!(
            super::confident_explicit_ranking(&request, &[-3.0, 0.0, -0.5], &baseline).is_none()
        );
        assert_eq!(
            super::confident_explicit_ranking(&request, &[-3.0, 0.0, -2.0], &baseline).unwrap()[0],
            "重傷を負う"
        );
        assert!(
            super::confident_explicit_ranking(&request, &[f64::NAN, 0.0, -2.0], &baseline)
                .is_none()
        );
        assert!(super::confident_explicit_ranking(&request, &[-3.0, 0.0], &baseline).is_none());
    }

    #[test]
    fn explicit_confidence_preserves_numbers_and_hiragana() {
        for (before, after) in [("30も半ば", "三重も半ば"), ("重症を負う", "重傷をおう")]
        {
            let request = CandidateRankingRequest {
                reading: "てすと".to_owned(),
                left_context: String::new(),
                candidates: [before, after]
                    .into_iter()
                    .map(|surface| CandidateRankingItem {
                        surface: surface.to_owned(),
                        cost: 100,
                    })
                    .collect(),
            };
            assert!(
                super::confident_explicit_ranking(&request, &[-3.0, 0.0], &[before.to_owned()])
                    .is_none()
            );
        }
    }

    #[test]
    fn live_neural_ranking_rejects_a_deep_short_tail_switch() {
        let mut request = CandidateRankingRequest {
            reading: "ひがしほうこうにのび".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "東方向に伸び".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "東方向に野火".to_owned(),
                    cost: 3_600,
                },
            ],
        };

        assert!(
            ranked_candidate_surfaces(
                &request,
                &[0.0, 0.4],
                1.0,
                0.2,
                SurfaceLengthPolicy::Preserve,
                None,
                None,
            )
            .is_none(),
            "a weak model preference must not override a much stronger dictionary tail"
        );
        assert!(
            ranked_candidate_surfaces(
                &request,
                &[0.0, 0.6],
                1.0,
                0.2,
                SurfaceLengthPolicy::Preserve,
                None,
                None,
            )
            .is_none(),
            "even a decisive model preference must not resurrect a deep two-character tail"
        );

        request.candidates[1].cost = 2_900;
        assert_eq!(
            ranked_candidate_surfaces(
                &request,
                &[0.0, 0.6],
                1.0,
                0.2,
                SurfaceLengthPolicy::Preserve,
                None,
                None,
            )
            .unwrap()[0],
            "東方向に野火",
            "the ordinary candidate band remains under the model's control"
        );
    }

    #[test]
    fn live_neural_ranking_rejects_only_a_deep_in_compound_substitution() {
        assert!(rewrites_one_han_inside_compound(
            "遊技台選択時",
            "遊戯台選択時"
        ));
        assert!(!rewrites_one_han_inside_compound(
            "公平性を書いた記事",
            "公平性を欠いた記事"
        ));
        assert!(!rewrites_one_han_inside_compound(
            "専用運用感",
            "専用運用艦"
        ));
        assert!(!rewrites_one_han_inside_compound("区形状", "矩形状"));
        assert!(!rewrites_one_han_inside_compound("再販する", "再版する"));

        let mut request = CandidateRankingRequest {
            reading: "ゆうぎだいせんたくじ".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "遊技台選択時".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "遊戯台選択時".to_owned(),
                    cost: 1_400,
                },
            ],
        };
        assert!(
            ranked_candidate_surfaces(
                &request,
                &[0.0, 1.0],
                1.0,
                0.2,
                SurfaceLengthPolicy::Preserve,
                None,
                None,
            )
            .is_none()
        );

        request.candidates[1].cost = 1_399;
        assert_eq!(
            ranked_candidate_surfaces(
                &request,
                &[0.0, 1.0],
                1.0,
                0.2,
                SurfaceLengthPolicy::Preserve,
                None,
                None,
            )
            .unwrap()[0],
            "遊戯台選択時"
        );
    }

    #[test]
    fn live_neural_ranking_uses_stronger_weight_for_long_readings() {
        assert!((live_neural_lambda(0.2, 0.3, "せいど") - 0.2).abs() < f64::EPSILON);
        assert!((live_neural_lambda(0.2, 0.3, "へんかんせいど") - 0.3).abs() < f64::EPSILON);
        assert!((live_neural_switch_margin(0.2, 0.4, "せいど") - 0.2).abs() < f64::EPSILON);
        assert!((live_neural_switch_margin(0.2, 0.4, "へんかんせいど") - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn newly_scored_explicit_switches_preserve_literal_hiragana() {
        assert!(super::explicit_switch_preserves_hiragana(
            "志願して出生していく",
            "志願して出征していく"
        ));
        assert!(super::explicit_switch_preserves_hiragana(
            "大学）終了",
            "大学）修了"
        ));
        assert!(!super::explicit_switch_preserves_hiragana(
            "誕生とし大いに喜んだ",
            "誕生年大いに喜んだ"
        ));
        assert!(!super::explicit_switch_preserves_hiragana(
            "地球から離れ",
            "地球から離"
        ));
    }

    #[test]
    fn explicit_cost_gap_setter_rejects_invalid_or_unavailable_handles() {
        // SAFETY: Null is checked before dereference.
        assert_eq!(
            unsafe { super::slime_set_explicit_neural_cost_gap(std::ptr::null_mut(), 1500) },
            super::STATUS_NULL_HANDLE
        );
        let handle = super::slime_create();
        assert!(!handle.is_null());
        // SAFETY: The test exclusively owns this live handle until destruction.
        unsafe {
            assert_eq!(
                super::slime_set_explicit_neural_cost_gap(handle, -1),
                super::STATUS_INVALID_COST_GAP
            );
            assert_eq!(
                super::slime_set_explicit_neural_cost_gap(handle, 1500),
                super::STATUS_NEURAL_UNAVAILABLE
            );
            super::slime_destroy(handle);
        }
    }

    #[test]
    fn explicit_weight_is_opt_in_and_counts_unicode_characters() {
        for (length, expected) in [(19, 0.2), (20, 0.4), (21, 0.4)] {
            let reading = "あ".repeat(length);
            let actual = super::explicit_neural_lambda(0.2, Some((20, 0.4)), None, &reading);
            assert!((actual - expected).abs() < f64::EPSILON);
            assert!(
                (super::explicit_neural_lambda(0.2, None, None, &reading) - 0.2).abs()
                    < f64::EPSILON
            );
        }
    }

    #[test]
    fn explicit_medium_weight_preserves_short_and_long_readings() {
        for (long_reading_weight, length, expected) in [
            (Some((20, 0.45)), 0, 0.2),
            (Some((20, 0.45)), 2, 0.2),
            (Some((20, 0.45)), 3, 0.3),
            (Some((20, 0.45)), 19, 0.3),
            (Some((20, 0.45)), 20, 0.45),
            // The long-reading weight wins where both thresholds apply.
            (Some((3, 0.4)), 3, 0.4),
            // Medium weight alone never applies to long readings.
            (None, 20, 0.2),
        ] {
            let reading = "あ".repeat(length);
            let actual =
                super::explicit_neural_lambda(0.2, long_reading_weight, Some(0.3), &reading);
            assert!(
                (actual - expected).abs() < f64::EPSILON,
                "{long_reading_weight:?} {length}: {actual}"
            );
        }
    }

    #[test]
    fn explicit_medium_weight_validates_ffi_inputs() {
        let handle = super::slime_create();
        assert!(!handle.is_null());
        // SAFETY: This test exclusively owns the handle until destruction.
        unsafe {
            assert_eq!(
                super::slime_set_explicit_neural_medium_reading_weight(std::ptr::null_mut(), 0.3),
                super::STATUS_NULL_HANDLE
            );
            for weight in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
                assert_eq!(
                    super::slime_set_explicit_neural_medium_reading_weight(handle, weight),
                    super::STATUS_INVALID_WEIGHT
                );
            }
            assert_eq!(
                super::slime_set_explicit_neural_medium_reading_weight(handle, 0.3),
                super::STATUS_NEURAL_UNAVAILABLE
            );
            super::slime_destroy(handle);
        }
    }

    #[test]
    fn live_lambda_fallback_requires_a_long_target_and_distinct_weight() {
        assert!(!should_try_live_lambda_fallback("うかがえる", 0.6, 0.2));
        assert!(should_try_live_lambda_fallback(
            "しかいをつとめ、じしんのぶろぐでもそのちょうか",
            0.6,
            0.2
        ));
        assert!(!should_try_live_lambda_fallback(
            "しかいをつとめ、じしんのぶろぐでもそのちょうか",
            0.2,
            0.2
        ));
    }

    #[test]
    fn live_neural_ranking_detects_a_bounded_name_spelling_family() {
        let name = CandidateRankingRequest {
            reading: "よしひこ".to_owned(),
            left_context: "野田".to_owned(),
            candidates: ["佳彦", "慶彦", "美彦", "吉彦"]
                .into_iter()
                .enumerate()
                .map(|(index, surface)| CandidateRankingItem {
                    surface: surface.to_owned(),
                    cost: i32::try_from(index).expect("four fixtures fit in i32"),
                })
                .collect(),
        };
        assert!(candidate_family_is_name_spelling_ambiguity(&name));

        let ordinary_word = CandidateRankingRequest {
            reading: "せいど".to_owned(),
            left_context: "変換".to_owned(),
            candidates: ["制度", "精度", "聖堂", "せいど"]
                .into_iter()
                .enumerate()
                .map(|(index, surface)| CandidateRankingItem {
                    surface: surface.to_owned(),
                    cost: i32::try_from(index).expect("four fixtures fit in i32"),
                })
                .collect(),
        };
        assert!(!candidate_family_is_name_spelling_ambiguity(&ordinary_word));
    }

    #[test]
    fn rejected_neural_ranking_uses_dictionary_base_only_for_long_unresolved_targets() {
        let long_literal = CandidateRankingRequest {
            reading: "ゆうぎだいせんたくじ".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "遊技台選択時".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "ゆうぎだいせんたくじ".to_owned(),
                    cost: i32::MAX,
                },
            ],
        };
        assert!(should_use_dictionary_base_after_rejected_neural(
            &long_literal,
            true
        ));
        assert!(!should_use_dictionary_base_after_rejected_neural(
            &long_literal,
            false
        ));

        let short_ambiguous = CandidateRankingRequest {
            reading: "まち".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "街".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "まち".to_owned(),
                    cost: i32::MAX,
                },
            ],
        };
        assert!(!should_use_dictionary_base_after_rejected_neural(
            &short_ambiguous,
            true
        ));

        let protected_colloquial = CandidateRankingRequest {
            reading: "おもろい".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "オモロイ".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "おもろい".to_owned(),
                    cost: 600,
                },
            ],
        };
        assert!(!should_use_dictionary_base_after_rejected_neural(
            &protected_colloquial,
            true
        ));
    }

    #[test]
    fn rejected_neural_ranking_uses_a_structurally_safe_inflected_dictionary_base() {
        let inflected_base = CandidateRankingRequest {
            reading: "かける".to_owned(),
            left_context: "是非".to_owned(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "書ける".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "かける".to_owned(),
                    cost: 600,
                },
            ],
        };
        assert!(should_use_dictionary_base_after_rejected_neural(
            &inflected_base,
            true
        ));

        let short_inflected_base = CandidateRankingRequest {
            reading: "いたら".to_owned(),
            left_context: "ラッシュフォードは".to_owned(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "至ら".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "いたら".to_owned(),
                    cost: 600,
                },
            ],
        };
        assert!(!should_use_dictionary_base_after_rejected_neural(
            &short_inflected_base,
            true
        ));

        let compressed_inflected_base = CandidateRankingRequest {
            reading: "うかがえる".to_owned(),
            left_context: "人使いの上手い人物であることが".to_owned(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "伺える".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "うかがえる".to_owned(),
                    cost: 600,
                },
            ],
        };
        assert!(!should_use_dictionary_base_after_rejected_neural(
            &compressed_inflected_base,
            true
        ));

        let multi_kanji_prefix = CandidateRankingRequest {
            reading: "せいさくしている".to_owned(),
            left_context: "トランスコアを".to_owned(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "制作している".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "せいさくしている".to_owned(),
                    cost: 600,
                },
            ],
        };
        assert!(should_use_dictionary_base_after_rejected_neural(
            &multi_kanji_prefix,
            true
        ));

        let colloquial_phrase = CandidateRankingRequest {
            reading: "とにかくめちゃくちゃになってます".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "とにかく滅茶苦茶になってます".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "とにかくめちゃくちゃになってます".to_owned(),
                    cost: 600,
                },
            ],
        };
        assert!(!should_use_dictionary_base_after_rejected_neural(
            &colloquial_phrase,
            true
        ));
    }

    #[test]
    fn live_neural_ranking_requires_margin_when_the_dictionary_base_wins() {
        let request = CandidateRankingRequest {
            reading: "うかがえる".to_owned(),
            left_context: "人物であることが".to_owned(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "伺える".to_owned(),
                    cost: 6_187,
                },
                CandidateRankingItem {
                    surface: "窺える".to_owned(),
                    cost: 6_569,
                },
            ],
        };
        let logliks = [-31.865_884, -30.812_596];

        assert!(
            ranked_candidate_surfaces(
                &request,
                &logliks,
                0.3,
                0.3,
                SurfaceLengthPolicy::Preserve,
                None,
                None
            )
            .is_none()
        );
        assert_eq!(
            ranked_candidate_surfaces(
                &request,
                &logliks,
                0.3,
                0.2,
                SurfaceLengthPolicy::Preserve,
                None,
                None
            )
            .unwrap()[0],
            "伺える"
        );
    }

    #[test]
    fn live_neural_ranking_can_relax_only_for_a_dictionary_base_repair() {
        let request = CandidateRankingRequest {
            reading: "いたい".to_owned(),
            left_context: "荼毘に付された".to_owned(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "遺体".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "異体".to_owned(),
                    cost: 100,
                },
            ],
        };

        assert!(
            ranked_candidate_surfaces(
                &request,
                &[0.0, -0.15],
                1.0,
                0.3,
                SurfaceLengthPolicy::Preserve,
                None,
                None
            )
            .is_none()
        );
        assert_eq!(
            ranked_candidate_surfaces(
                &request,
                &[0.0, -0.15],
                1.0,
                0.3,
                SurfaceLengthPolicy::Preserve,
                None,
                Some(0.1),
            )
            .unwrap()[0],
            "遺体"
        );
        assert!(
            ranked_candidate_surfaces(
                &request,
                &[-0.15, 0.0],
                1.0,
                0.3,
                SurfaceLengthPolicy::Preserve,
                None,
                Some(0.1),
            )
            .is_none(),
            "the relaxed gate must not let the model override dictionary top-1"
        );
    }

    #[test]
    fn live_neural_ranking_can_repair_an_implicit_numeric_base_candidate() {
        let mut request = CandidateRankingRequest {
            reading: "いちこてい".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "1固定".to_owned(),
                    cost: 8_502,
                },
                CandidateRankingItem {
                    surface: "位置固定".to_owned(),
                    cost: 9_386,
                },
            ],
        };
        let logliks = [-28.327_862, -23.697_815];
        assert!(dictionary_base_introduces_implicit_numeric(&request));
        assert_eq!(
            ranked_candidate_surfaces(
                &request,
                &logliks,
                0.3,
                0.3,
                SurfaceLengthPolicy::Preserve,
                Some(0.1),
                None,
            )
            .unwrap()[0],
            "位置固定"
        );
        assert!(
            ranked_candidate_surfaces(
                &request,
                &logliks,
                0.3,
                0.3,
                SurfaceLengthPolicy::Preserve,
                None,
                None
            )
            .is_none()
        );
        request.reading = "1こてい".to_owned();
        assert!(!dictionary_base_introduces_implicit_numeric(&request));
        assert!(
            ranked_candidate_surfaces(
                &request,
                &logliks,
                0.3,
                0.3,
                SurfaceLengthPolicy::Preserve,
                Some(0.1),
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn live_neural_ranking_repairs_a_numeric_homophone_in_a_long_reading() {
        let request = CandidateRankingRequest {
            reading: "てんたいしょうのいち".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "点対称の1".to_owned(),
                    cost: 12_558,
                },
                CandidateRankingItem {
                    surface: "点対称の位置".to_owned(),
                    cost: 13_269,
                },
            ],
        };
        let logliks = [-31.331_263, -26.266_268];

        assert_eq!(
            ranked_candidate_surfaces(
                &request,
                &logliks,
                0.3,
                0.3,
                SurfaceLengthPolicy::Preserve,
                Some(0.1),
                None,
            )
            .unwrap()[0],
            "点対称の位置"
        );
        assert!(
            ranked_candidate_surfaces(
                &request,
                &logliks,
                0.2,
                0.3,
                SurfaceLengthPolicy::Preserve,
                Some(0.1),
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn neural_cost_gap_gate_scores_only_ambiguous_base_rankings() {
        let request = CandidateRankingRequest {
            reading: "てすと".to_owned(),
            left_context: String::new(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "第一候補".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "第二候補".to_owned(),
                    cost: 1_100,
                },
                CandidateRankingItem {
                    surface: "第三候補".to_owned(),
                    cost: 1_500,
                },
            ],
        };

        assert_eq!(base_cost_gap(&request), Some(1_000));
        assert!(should_score_neurally(&request, None));
        assert!(should_score_neurally(&request, Some(1_000)));
        assert!(!should_score_neurally(&request, Some(999)));
        assert!(!should_score_live_neurally(&request, Some(999)));
        let mut long_request = request.clone();
        long_request.reading = "あ".repeat(LIVE_RELAXED_COST_GAP_MIN_TARGET_CHARACTERS);
        assert!(should_score_live_neurally(&long_request, Some(999)));
        long_request.candidates[1].cost = 2_101;
        long_request.candidates[2].cost = 2_200;
        assert!(!should_score_live_neurally(&long_request, Some(999)));
        assert_eq!(
            base_ranked_candidate_surfaces(&request),
            ["第一候補", "第二候補", "第三候補"]
        );
    }

    #[test]
    fn live_contextual_cost_gate_keeps_length_and_gap_boundaries() {
        let mut request = CandidateRankingRequest {
            reading: "あ".repeat(6),
            left_context: "文脈".to_owned(),
            candidates: vec![
                CandidateRankingItem {
                    surface: "候補".to_owned(),
                    cost: 100,
                },
                CandidateRankingItem {
                    surface: "別候補".to_owned(),
                    cost: 2_100,
                },
            ],
        };
        assert!(should_score_live_neurally(&request, Some(1_000)));
        assert!(!should_score_neurally(&request, Some(1_000)));
        request.candidates[1].cost = 2_101;
        assert!(!should_score_live_neurally(&request, Some(1_000)));
        assert!(should_score_live_neurally(&request, None));
        request.candidates[1].cost = 2_100;
        request.reading.pop();
        assert!(!should_score_live_neurally(&request, Some(1_000)));
        request.reading.push('あ');
        request.left_context.clear();
        assert!(!should_score_live_neurally(&request, Some(1_000)));
        request.reading = "あ".repeat(10);
        assert!(should_score_live_neurally(&request, Some(1_000)));
        request.candidates[1].cost = 1_900;
        assert!(should_score_live_neurally(&request, Some(2_000)));
    }

    #[cfg(feature = "neural")]
    #[test]
    fn contextual_live_suffix_can_change_length_with_confidence() {
        let mut engine = super::SlimeEngine::bundled();
        engine.set_preferences(slime_core::EnginePreferences {
            live_conversion: true,
            ..slime_core::EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        for character in "いえにはやくかえる".chars() {
            engine.handle(slime_core::InputEvent::Character(character));
        }
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let mut request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, "はやくかえる");
        assert_eq!(request.left_context, "家に");
        assert!(matches!(
            super::live_surface_length_policy(&snapshot, &request),
            SurfaceLengthPolicy::AllowWithMargin(0.5)
        ));
        request.left_context.clear();
        assert!(matches!(
            super::live_surface_length_policy(&snapshot, &request),
            SurfaceLengthPolicy::Preserve
        ));
        request.left_context = "家に".to_owned();
        request.reading.pop();
        assert!(matches!(
            super::live_surface_length_policy(&snapshot, &request),
            SurfaceLengthPolicy::Preserve
        ));
    }

    #[cfg(feature = "neural")]
    #[test]
    fn contextual_base_confirmation_requires_a_bounded_changed_han_target() {
        let mut engine = super::SlimeEngine::bundled();
        engine.set_preferences(slime_core::EnginePreferences {
            live_conversion: true,
            ..slime_core::EnginePreferences::default()
        });
        engine.set_delayed_live_ranking_available(true);
        for c in "ginkoukou".chars() {
            engine.handle(slime_core::InputEvent::Character(c));
        }
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        let mut ranked: Vec<_> = request
            .candidates
            .iter()
            .map(|c| c.surface.clone())
            .collect();
        let index = ranked.iter().position(|s| s == "銀高校").unwrap();
        let chosen = ranked.remove(index);
        ranked.insert(0, chosen);
        assert!(
            engine
                .apply_live_candidate_ranking(&snapshot, &request, &ranked)
                .is_some()
        );
        for c in "zanokaisetsu".chars() {
            engine.handle(slime_core::InputEvent::Character(c));
        }
        let snapshot = engine.live_candidate_ranking_snapshot().unwrap();
        let request = snapshot.candidate_ranking_request().unwrap();
        assert_eq!(request.reading, "かいせつ");
        assert_eq!(request.candidates[0].surface, "開設");
        assert!(!should_score_live_neurally(&request, Some(1_000)));
        assert!(super::should_confirm_live_contextual_base(
            &snapshot, &request
        ));
        let mut without_context = request.clone();
        without_context.left_context.clear();
        assert!(!super::should_confirm_live_contextual_base(
            &snapshot,
            &without_context
        ));
        let mut unchanged = request.clone();
        unchanged.candidates[0].surface = "解説".to_owned();
        assert!(!super::should_confirm_live_contextual_base(
            &snapshot, &unchanged
        ));
        let mut expensive = request.clone();
        expensive.candidates.truncate(2);
        expensive.candidates[1].cost = expensive.candidates[0].cost + 2_501;
        assert!(!super::should_confirm_live_contextual_base(
            &snapshot, &expensive
        ));
        expensive.candidates[1].cost -= 1;
        assert!(super::should_confirm_live_contextual_base(
            &snapshot, &expensive
        ));
    }

    #[cfg(feature = "neural")]
    #[test]
    fn live_neural_task_can_move_to_an_independent_worker() {
        fn assert_send<T: Send>() {}
        assert_send::<super::SlimeLiveNeuralTask>();
    }

    #[test]
    fn null_live_neural_task_has_no_reading_characters() {
        assert_eq!(
            // SAFETY: Null is an explicitly supported diagnostic input.
            unsafe { super::slime_live_neural_task_reading_character_count(std::ptr::null()) },
            0
        );
    }

    #[test]
    fn neural_reranker_validates_weight_before_loading() {
        let handle = slime_create();
        let path = b"missing.gguf";
        assert_eq!(
            // SAFETY: The handle and path are live for the duration of the call.
            unsafe {
                slime_enable_neural_reranker(handle, path.as_ptr(), path.len(), f64::INFINITY)
            },
            STATUS_INVALID_WEIGHT
        );
        // SAFETY: The handle is live and uniquely owned by this test.
        unsafe { slime_destroy(handle) };
    }

    #[test]
    fn neural_reranker_validates_cost_gap_before_loading() {
        let handle = slime_create();
        let path = b"missing.gguf";
        assert_eq!(
            // SAFETY: The handle and path are live for the duration of the call.
            unsafe {
                slime_enable_neural_reranker_with_cost_gap(
                    handle,
                    path.as_ptr(),
                    path.len(),
                    0.2,
                    -1,
                )
            },
            STATUS_INVALID_COST_GAP
        );
        // SAFETY: The handle is live and uniquely owned by this test.
        unsafe { slime_destroy(handle) };
    }

    #[cfg(not(feature = "neural"))]
    #[test]
    fn default_build_reports_neural_reranker_as_unavailable() {
        let handle = slime_create();
        let path = b"model.gguf";
        assert_eq!(
            // SAFETY: The handle and path are live for the duration of the call.
            unsafe { slime_enable_neural_reranker(handle, path.as_ptr(), path.len(), 0.2) },
            STATUS_NEURAL_UNAVAILABLE
        );
        assert!(
            // SAFETY: The handle is live and accessed synchronously.
            unsafe { slime_live_neural_task_create(handle, 0.5, 0.1, 0.2) }.is_null()
        );
        assert_eq!(
            // SAFETY: A null task is explicitly accepted and reported.
            unsafe { slime_live_neural_task_run(std::ptr::null_mut()) },
            STATUS_NULL_HANDLE
        );
        // SAFETY: A null task is accepted by the destructor.
        unsafe { slime_live_neural_task_destroy(std::ptr::null_mut()) };
        assert_eq!(
            // SAFETY: The handle and path are live for the duration of the call.
            unsafe {
                slime_enable_neural_reranker_with_cost_gap(
                    handle,
                    path.as_ptr(),
                    path.len(),
                    0.2,
                    1_000,
                )
            },
            STATUS_NEURAL_UNAVAILABLE
        );
        // SAFETY: The handle is live and uniquely owned by this test.
        unsafe { slime_destroy(handle) };
    }

    #[cfg(not(feature = "neural"))]
    #[test]
    fn default_build_rejects_delayed_neural_ranking_but_can_disable_it() {
        let handle = slime_create();
        assert!(!handle.is_null());
        // SAFETY: The handle is live and exclusively accessed.
        assert_eq!(
            unsafe { slime_set_live_neural_ranking_enabled(handle, true) },
            STATUS_NEURAL_UNAVAILABLE
        );
        // SAFETY: The handle remains live and exclusively accessed.
        assert_eq!(
            unsafe { slime_set_live_neural_ranking_enabled(handle, false) },
            STATUS_OK
        );
        // SAFETY: The handle is live and uniquely owned.
        unsafe { slime_destroy(handle) };
    }

    unsafe fn copy_buffer(buffer: &SlimeBuffer) -> String {
        // SAFETY: Tests read a live buffer before handing it back to its destructor.
        let bytes = unsafe { std::slice::from_raw_parts(buffer.data, buffer.len) };
        String::from_utf8(bytes.to_vec()).unwrap()
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
