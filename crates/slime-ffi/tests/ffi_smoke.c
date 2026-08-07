#include "slime_ffi.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

typedef struct TypedActions {
  size_t preedit_count;
  size_t candidate_count;
  bool saw_show_candidates;
  char last_preedit[256];
} TypedActions;

typedef struct CandidateCapture {
  size_t count;
  bool found_nihon;
} CandidateCapture;

static void collect_action(void *context, const SlimeActionView *action) {
  TypedActions *collected = context;
  assert(action != NULL);
  switch (action->kind) {
  case SLIME_ACTION_UPDATE_PREEDIT:
    assert(action->text.data != NULL);
    assert(action->text.len > 0);
    assert(action->text.len < sizeof(collected->last_preedit));
    memcpy(collected->last_preedit, action->text.data, action->text.len);
    collected->last_preedit[action->text.len] = '\0';
    collected->preedit_count += 1;
    break;
  case SLIME_ACTION_SHOW_CANDIDATES:
    assert(action->candidates != NULL);
    assert(action->candidate_count > 0);
    assert(action->selected < action->candidate_count);
    collected->candidate_count = action->candidate_count;
    collected->saw_show_candidates = true;
    break;
  default:
    break;
  }
}

static void collect_candidate(void *context, SlimeStringView value) {
  CandidateCapture *capture = context;
  const char nihon[] = "日本";
  capture->count += 1;
  if (value.len == sizeof(nihon) - 1 &&
      memcmp(value.data, nihon, sizeof(nihon) - 1) == 0) {
    capture->found_nihon = true;
  }
}

int main(void) {
  SlimeHandle *handle = slime_create();
  assert(handle != NULL);

  const char *input = "nihon";
  for (size_t index = 0; input[index] != '\0'; ++index) {
    SlimeBuffer response =
        slime_process(handle, SLIME_EVENT_CHARACTER, (uint32_t)input[index]);
    assert(response.data != NULL);
    slime_buffer_destroy(response);
  }

  SlimeBuffer response = slime_process(handle, SLIME_EVENT_SPACE, 0);
  assert(response.data != NULL);

  char json[1024];
  assert(response.len < sizeof(json));
  memcpy(json, response.data, response.len);
  json[response.len] = '\0';
  assert(strstr(json, "show_candidates") != NULL);

  slime_buffer_destroy(response);
  slime_destroy(handle);

  handle = slime_create();
  assert(handle != NULL);
  TypedActions actions = {0};
  for (size_t index = 0; input[index] != '\0'; ++index) {
    assert(slime_process_actions(handle, SLIME_EVENT_CHARACTER,
                                 (uint32_t)input[index], &actions,
                                 collect_action) == SLIME_STATUS_OK);
  }
  assert(slime_process_actions(handle, SLIME_EVENT_SPACE, 0, &actions,
                               collect_action) == SLIME_STATUS_OK);
  assert(actions.preedit_count > 0);
  assert(actions.saw_show_candidates);
  assert(actions.candidate_count > 0);
  assert(slime_process_actions(NULL, SLIME_EVENT_SPACE, 0, &actions,
                               collect_action) == SLIME_STATUS_NULL_HANDLE);
  assert(slime_process_actions(handle, SLIME_EVENT_SPACE, 0, &actions, NULL) ==
         SLIME_STATUS_NULL_CALLBACK);
  slime_destroy(handle);

  handle = slime_create();
  assert(handle != NULL);
  const char *reading = "にほん";
  const char *surface = "日本";
  CandidateCapture candidate_capture = {0};
  assert(slime_conversion_candidates(
             handle, (const uint8_t *)reading, strlen(reading),
             &candidate_capture, collect_candidate) == SLIME_STATUS_OK);
  assert(candidate_capture.count > 0);
  assert(candidate_capture.found_nihon);
  assert(slime_record_external_selection(
             handle, (const uint8_t *)reading, strlen(reading),
             (const uint8_t *)surface, strlen(surface)) == SLIME_STATUS_OK);
  const char *invalid_surface = "無関係";
  assert(slime_record_external_selection(
             handle, (const uint8_t *)reading, strlen(reading),
             (const uint8_t *)invalid_surface, strlen(invalid_surface)) ==
         SLIME_STATUS_INVALID_CANDIDATE);
  slime_destroy(handle);

  handle = slime_create();
  assert(handle != NULL);
  response = slime_set_options(handle, true, false);
  slime_buffer_destroy(response);
  const char *live_input = "raibuhenkannno";
  TypedActions live_actions = {0};
  for (size_t index = 0; live_input[index] != '\0'; ++index) {
    assert(slime_process_actions(handle, SLIME_EVENT_CHARACTER,
                                 (uint32_t)live_input[index], &live_actions,
                                 collect_action) == SLIME_STATUS_OK);
  }
  assert(strcmp(live_actions.last_preedit, "ライブ変換の") == 0);
  slime_destroy(handle);

  puts("C ABI smoke test passed");
  return 0;
}
