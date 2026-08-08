#ifndef SLIME_FFI_H
#define SLIME_FFI_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct SlimeHandle SlimeHandle;

typedef struct SlimeBuffer {
  uint8_t *data;
  size_t len;
  size_t capacity;
} SlimeBuffer;

typedef struct SlimeStringView {
  const uint8_t *data;
  size_t len;
} SlimeStringView;

typedef struct SlimeActionView {
  uint32_t kind;
  SlimeStringView text;
  const SlimeStringView *candidates;
  size_t candidate_count;
  size_t selected;
  size_t selection_start;
  size_t selection_length;
} SlimeActionView;

typedef struct SlimeCandidateViewV2 {
  SlimeStringView value;
  SlimeStringView display;
  uint32_t annotation;
  SlimeStringView detail;
} SlimeCandidateViewV2;

typedef struct SlimeActionViewV2 {
  uint32_t kind;
  SlimeStringView text;
  const SlimeCandidateViewV2 *candidates;
  size_t candidate_count;
  size_t selected;
  size_t selection_start;
  size_t selection_length;
} SlimeActionViewV2;

typedef void (*SlimeActionCallback)(void *context,
                                    const SlimeActionView *action);
typedef void (*SlimeActionCallbackV2)(void *context,
                                      const SlimeActionViewV2 *action);
typedef void (*SlimeStringCallback)(void *context, SlimeStringView value);

enum SlimeEventKind {
  SLIME_EVENT_CHARACTER = 0,
  SLIME_EVENT_SPACE = 1,
  SLIME_EVENT_ENTER = 2,
  SLIME_EVENT_ESCAPE = 3,
  SLIME_EVENT_BACKSPACE = 4,
  SLIME_EVENT_NEXT_CANDIDATE = 5,
  SLIME_EVENT_PREVIOUS_CANDIDATE = 6,
  SLIME_EVENT_SELECT_CANDIDATE = 7,
  SLIME_EVENT_ACCEPT_CANDIDATE = 8,
  SLIME_EVENT_TRANSFORM_HIRAGANA = 9,
  SLIME_EVENT_TRANSFORM_FULL_KATAKANA = 10,
  SLIME_EVENT_TRANSFORM_HALF_KATAKANA = 11,
  SLIME_EVENT_TRANSFORM_FULL_ALPHANUMERIC = 12,
  SLIME_EVENT_TRANSFORM_HALF_ALPHANUMERIC = 13,
  SLIME_EVENT_NEXT_SEGMENT = 14,
  SLIME_EVENT_PREVIOUS_SEGMENT = 15,
  SLIME_EVENT_EXPAND_SEGMENT = 16,
  SLIME_EVENT_SHRINK_SEGMENT = 17,
};

enum SlimeActionKind {
  SLIME_ACTION_UPDATE_PREEDIT = 0,
  SLIME_ACTION_SHOW_CANDIDATES = 1,
  SLIME_ACTION_HIDE_CANDIDATES = 2,
  SLIME_ACTION_COMMIT = 3,
  SLIME_ACTION_CLEAR = 4,
  SLIME_ACTION_FORWARD_KEY = 5,
};

enum SlimeCandidateAnnotation {
  SLIME_CANDIDATE_ANNOTATION_NONE = 0,
  SLIME_CANDIDATE_ANNOTATION_USER_DICTIONARY = 1,
  SLIME_CANDIDATE_ANNOTATION_HISTORY = 2,
  SLIME_CANDIDATE_ANNOTATION_CORRECTION = 3,
  SLIME_CANDIDATE_ANNOTATION_COMPLETION = 4,
  SLIME_CANDIDATE_ANNOTATION_DATE_TIME = 5,
  SLIME_CANDIDATE_ANNOTATION_NUMBER = 6,
  SLIME_CANDIDATE_ANNOTATION_CONTEXT = 7,
};

enum SlimeStatus {
  SLIME_STATUS_OK = 0,
  SLIME_STATUS_NULL_HANDLE = 1,
  SLIME_STATUS_INVALID_EVENT = 2,
  SLIME_STATUS_NULL_CALLBACK = 3,
  SLIME_STATUS_PANIC = 4,
  SLIME_STATUS_INVALID_UTF8 = 5,
  SLIME_STATUS_INVALID_CANDIDATE = 6,
};

SlimeHandle *slime_create(void);
SlimeHandle *slime_create_with_data_dir(const uint8_t *data_dir,
                                    size_t data_dir_len);
/* verification_keys contains one
   lowercase-key-id<TAB>64-lowercase-hex-ed25519-public-key row per key.
   Every installed dictionary pack must have a valid .slime-dict.sig sidecar. */
SlimeHandle *slime_create_with_signed_data_dir(
    const uint8_t *data_dir, size_t data_dir_len,
    const uint8_t *verification_keys, size_t verification_keys_len);
/* version_floors contains one
   lowercase-pack-id<TAB>MAJOR.MINOR.PATCH row per rollback-protected pack. */
SlimeHandle *slime_create_with_signed_data_dir_and_version_floors(
    const uint8_t *data_dir, size_t data_dir_len,
    const uint8_t *verification_keys, size_t verification_keys_len,
    const uint8_t *version_floors, size_t version_floors_len);
void slime_destroy(SlimeHandle *handle);
SlimeBuffer slime_process(SlimeHandle *handle, uint32_t event_kind, uint32_t value);
/* Calls callback synchronously for each action. All views are borrowed only
   for that callback; the callback must not retain them, unwind, or re-enter
   this handle. Existing JSON callers can continue using slime_process. */
uint32_t slime_process_actions(SlimeHandle *handle, uint32_t event_kind,
                               uint32_t value, void *context,
                               SlimeActionCallback callback);
/* v2 keeps the v1 action layout intact and adds per-candidate committed value,
   legacy display, semantic annotation, and optional detail. */
uint32_t slime_process_actions_v2(SlimeHandle *handle, uint32_t event_kind,
                                  uint32_t value, void *context,
                                  SlimeActionCallbackV2 callback);
/* Enumerates candidates synchronously without changing composition state.
   Views are borrowed for one callback and must not be retained. */
uint32_t slime_conversion_candidates(const SlimeHandle *handle,
                                     const uint8_t *reading,
                                     size_t reading_len, void *context,
                                     SlimeStringCallback callback);
/* Learns a selection made by an external candidate consumer. The surface must
   be one of slime_conversion_candidates for the same reading. */
uint32_t slime_record_external_selection(SlimeHandle *handle,
                                         const uint8_t *reading,
                                         size_t reading_len,
                                         const uint8_t *surface,
                                         size_t surface_len);
SlimeBuffer slime_set_options(SlimeHandle *handle, bool live_conversion,
                          bool history_completion);
SlimeBuffer slime_set_options_v2(SlimeHandle *handle, bool live_conversion,
                             bool history_completion,
                             uint32_t dictionary_packs);
SlimeBuffer slime_set_options_v3(SlimeHandle *handle, bool live_conversion,
                             bool history_completion, bool history_learning,
                             uint32_t dictionary_packs);
SlimeBuffer slime_set_options_v4(SlimeHandle *handle, bool live_conversion,
                             bool history_completion, bool history_learning,
                             uint32_t dictionary_packs, bool private_mode);
SlimeBuffer slime_set_options_v5(SlimeHandle *handle, bool live_conversion,
                             bool history_completion, bool history_learning,
                             uint32_t dictionary_packs, bool private_mode,
                             uint32_t date_format_mask);
SlimeBuffer slime_begin_reconversion(SlimeHandle *handle,
                                 const uint8_t *surface, size_t surface_len);
/* Breaks transient left context after an external caret, document, or client
   boundary without deleting persisted history. */
uint32_t slime_reset_context(SlimeHandle *handle);
/* Supplies committed text immediately before the platform caret. The context
   is bounded, transient, never persisted, and ignored in private mode. */
uint32_t slime_set_external_left_context(SlimeHandle *handle,
                                         const uint8_t *surface,
                                         size_t surface_len);
SlimeBuffer slime_reload_user_data(SlimeHandle *handle);
SlimeBuffer slime_domain_dictionary_words(uint32_t mask);
SlimeBuffer slime_installed_dictionary_packs(const SlimeHandle *handle);
SlimeBuffer slime_installed_dictionary_pack_words(const SlimeHandle *handle,
                                                  const uint8_t *pack_id,
                                                  size_t pack_id_len);
void slime_buffer_destroy(SlimeBuffer buffer);

#ifdef __cplusplus
}
#endif

#endif
