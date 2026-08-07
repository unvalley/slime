cmake_minimum_required(VERSION 3.25)
project(SlimeIME LANGUAGES CXX)

include(CTest)

if(NOT WIN32)
  message(FATAL_ERROR "SlimeIME native shell must be built on Windows")
endif()

if(NOT DEFINED SLIME_FFI_LIBRARY)
  message(FATAL_ERROR "SLIME_FFI_LIBRARY must point to the Rust import library")
endif()

get_filename_component(SLIME_FFI_DIRECTORY "${SLIME_FFI_LIBRARY}" DIRECTORY)
set(SLIME_FFI_RUNTIME "${SLIME_FFI_DIRECTORY}/slime_ffi.dll")
if(NOT EXISTS "${SLIME_FFI_RUNTIME}")
  message(FATAL_ERROR "slime_ffi.dll must be beside the Rust import library")
endif()

add_library(SlimeIME SHARED
  src/CandidateWindow.cpp
  src/SearchCandidateList.cpp
  src/SlimeIME.cpp
  src/WindowsPreferences.cpp
  src/SlimeIME.def
)

add_executable(SlimeIMERegister src/RegisterIME.cpp)

add_executable(SlimeSettings WIN32
  src/WindowsPreferences.cpp
  src/WindowsSettings.cpp
)

add_executable(SlimeNativeTests
  src/CandidateWindow.cpp
  src/SearchCandidateList.cpp
  src/SearchCandidateListTests.cpp
  src/WindowsPreferences.cpp
)

target_compile_features(SlimeIME PRIVATE cxx_std_20)
target_compile_definitions(SlimeIME PRIVATE
  UNICODE
  _UNICODE
  WIN32_LEAN_AND_MEAN
  NOMINMAX
  _WIN32_WINNT=0x0A00
)
target_compile_options(SlimeIME PRIVATE /W4 /WX /permissive- /utf-8)
target_compile_features(SlimeIMERegister PRIVATE cxx_std_20)
target_compile_definitions(SlimeIMERegister PRIVATE
  UNICODE
  _UNICODE
  WIN32_LEAN_AND_MEAN
  NOMINMAX
  _WIN32_WINNT=0x0A00
)
target_compile_options(SlimeIMERegister PRIVATE /W4 /WX /permissive- /utf-8)
target_compile_features(SlimeSettings PRIVATE cxx_std_20)
target_compile_definitions(SlimeSettings PRIVATE
  UNICODE
  _UNICODE
  WIN32_LEAN_AND_MEAN
  NOMINMAX
  _WIN32_WINNT=0x0A00
)
target_compile_options(SlimeSettings PRIVATE /W4 /WX /permissive- /utf-8)
target_compile_features(SlimeNativeTests PRIVATE cxx_std_20)
target_compile_definitions(SlimeNativeTests PRIVATE
  UNICODE
  _UNICODE
  WIN32_LEAN_AND_MEAN
  NOMINMAX
  _WIN32_WINNT=0x0A00
)
target_compile_options(SlimeNativeTests PRIVATE /W4 /WX /permissive- /utf-8)
target_include_directories(SlimeIME PRIVATE
  "${CMAKE_CURRENT_SOURCE_DIR}/../../../crates/slime-ffi/include"
)
target_link_libraries(SlimeIME PRIVATE
  "${SLIME_FFI_LIBRARY}"
  advapi32
  gdi32
  ole32
  oleaut32
  user32
  uiautomationcore
  uuid
)
target_link_libraries(SlimeIMERegister PRIVATE shlwapi)
target_link_libraries(SlimeSettings PRIVATE gdi32 user32)
target_link_libraries(SlimeNativeTests PRIVATE
  gdi32
  oleaut32
  uiautomationcore
  user32
  uuid
)

add_custom_command(TARGET SlimeIME POST_BUILD
  COMMAND "${CMAKE_COMMAND}" -E copy_if_different
          "${SLIME_FFI_RUNTIME}"
          "$<TARGET_FILE_DIR:SlimeIME>/slime_ffi.dll"
)

add_test(NAME SlimeNativeTests COMMAND SlimeNativeTests)
set_tests_properties(SlimeNativeTests PROPERTIES
  WORKING_DIRECTORY "$<TARGET_FILE_DIR:SlimeIME>"
)

set_target_properties(SlimeIME PROPERTIES
  OUTPUT_NAME "SlimeIME"
  PREFIX ""
)
