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

SlimeHandle *slime_create(void);
SlimeHandle *slime_create_with_data_dir(const uint8_t *data_dir,
                                    size_t data_dir_len);
void slime_destroy(SlimeHandle *handle);
SlimeBuffer slime_process(SlimeHandle *handle, uint32_t event_kind, uint32_t value);
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
