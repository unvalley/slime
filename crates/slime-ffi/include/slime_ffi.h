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
SlimeBuffer slime_reload_user_data(SlimeHandle *handle);
SlimeBuffer slime_domain_dictionary_words(uint32_t mask);
void slime_buffer_destroy(SlimeBuffer buffer);

#ifdef __cplusplus
}
#endif

#endif
