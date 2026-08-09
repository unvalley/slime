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

typedef struct TypedActionsV2 {
  size_t candidate_count;
  bool saw_show_candidates;
  bool saw_separate_candidate_value;
} TypedActionsV2;

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

static void collect_action_v2(void *context, const SlimeActionViewV2 *action) {
  TypedActionsV2 *collected = context;
  assert(action != NULL);
  if (action->kind != SLIME_ACTION_SHOW_CANDIDATES) {
    return;
  }
  assert(action->candidates != NULL);
  assert(action->candidate_count > 0);
  for (size_t index = 0; index < action->candidate_count; ++index) {
    const SlimeCandidateViewV2 *candidate = &action->candidates[index];
    assert(candidate->value.data != NULL);
    assert(candidate->value.len > 0);
    assert(candidate->display.data != NULL);
    assert(candidate->display.len > 0);
    assert(candidate->annotation <= SLIME_CANDIDATE_ANNOTATION_CONTEXT);
  }
  collected->candidate_count = action->candidate_count;
  collected->saw_show_candidates = true;
  collected->saw_separate_candidate_value = true;
}

int main(void) {
  const char *signed_data_dir = "/tmp/slime-signed-data-dir-c-smoke";
  const char *verification_keys =
      "fixture-2026-a\t"
      "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a\n";
  SlimeHandle *signed_handle = slime_create_with_signed_data_dir(
      (const uint8_t *)signed_data_dir, strlen(signed_data_dir),
      (const uint8_t *)verification_keys, strlen(verification_keys));
  assert(signed_handle != NULL);
  slime_destroy(signed_handle);
  assert(slime_create_with_signed_data_dir(
             (const uint8_t *)signed_data_dir, strlen(signed_data_dir), NULL,
             0) == NULL);
  const char *version_floors = "sample-general\t2026.08.1\n";
  signed_handle = slime_create_with_signed_data_dir_and_version_floors(
      (const uint8_t *)signed_data_dir, strlen(signed_data_dir),
      (const uint8_t *)verification_keys, strlen(verification_keys),
      (const uint8_t *)version_floors, strlen(version_floors));
  assert(signed_handle != NULL);
  slime_destroy(signed_handle);
  assert(slime_create_with_signed_data_dir_and_version_floors(
             (const uint8_t *)signed_data_dir, strlen(signed_data_dir),
             (const uint8_t *)verification_keys, strlen(verification_keys),
             NULL, 0) == NULL);

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
  assert(slime_reset_context(handle) == SLIME_STATUS_OK);
  assert(slime_reset_context(NULL) == SLIME_STATUS_NULL_HANDLE);
  const char *left_context = "直前の文章";
  assert(slime_set_external_left_context(
             handle, (const uint8_t *)left_context, strlen(left_context)) ==
         SLIME_STATUS_OK);
  const uint8_t invalid_context[] = {0xff};
  assert(slime_set_external_left_context(handle, invalid_context,
                                         sizeof(invalid_context)) ==
         SLIME_STATUS_INVALID_UTF8);
  assert(slime_set_external_left_context(NULL, (const uint8_t *)left_context,
                                         strlen(left_context)) ==
         SLIME_STATUS_NULL_HANDLE);
  const char *right_context = "後続の文章";
  assert(slime_set_external_context(
             handle, (const uint8_t *)left_context, strlen(left_context),
             (const uint8_t *)right_context, strlen(right_context)) ==
         SLIME_STATUS_OK);
  assert(slime_set_external_context(handle, (const uint8_t *)left_context,
                                    strlen(left_context), invalid_context,
                                    sizeof(invalid_context)) ==
         SLIME_STATUS_INVALID_UTF8);
  slime_destroy(handle);

  handle = slime_create();
  assert(handle != NULL);
  TypedActionsV2 actions_v2 = {0};
  for (size_t index = 0; input[index] != '\0'; ++index) {
    assert(slime_process_actions_v2(handle, SLIME_EVENT_CHARACTER,
                                    (uint32_t)input[index], &actions_v2,
                                    collect_action_v2) == SLIME_STATUS_OK);
  }
  assert(slime_process_actions_v2(handle, SLIME_EVENT_SPACE, 0, &actions_v2,
                                  collect_action_v2) == SLIME_STATUS_OK);
  assert(actions_v2.saw_show_candidates);
  assert(actions_v2.saw_separate_candidate_value);
  assert(actions_v2.candidate_count > 0);
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
