#include "slime_ffi.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

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
  puts("C ABI smoke test passed");
  return 0;
}

