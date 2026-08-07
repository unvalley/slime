#!/usr/bin/env bash
set -euo pipefail

for target in x86_64-pc-windows-msvc i686-pc-windows-msvc; do
  if ! rustup target list --installed | grep -Fxq "$target"; then
    echo "missing Rust target: $target" >&2
    echo "install it with: rustup target add $target" >&2
    exit 1
  fi
  cargo check -p slime-ffi --target "$target"
done

for compiler in x86_64-w64-mingw32-g++ i686-w64-mingw32-g++; do
  if ! command -v "$compiler" >/dev/null 2>&1; then
    continue
  fi
  "$compiler" \
    -std=c++20 \
    -DUNICODE \
    -D_UNICODE \
    -DWIN32_LEAN_AND_MEAN \
    -DNOMINMAX \
    -D_WIN32_WINNT=0x0A00 \
    -Wall \
    -Wextra \
    -Werror \
    -Icrates/slime-ffi/include \
    -Iplatforms/windows/native/src \
    -fsyntax-only \
    platforms/windows/native/src/CandidateWindow.cpp \
    platforms/windows/native/src/SearchCandidateList.cpp \
    platforms/windows/native/src/SearchCandidateListTests.cpp \
    platforms/windows/native/src/SlimeIME.cpp \
    platforms/windows/native/src/WindowsPreferences.cpp
  "$compiler" \
    -std=c++20 \
    -DUNICODE \
    -D_UNICODE \
    -DWIN32_LEAN_AND_MEAN \
    -DNOMINMAX \
    -D_WIN32_WINNT=0x0A00 \
    -Wall \
    -Wextra \
    -Werror \
    -Iplatforms/windows/native/src \
    -fsyntax-only \
    platforms/windows/native/src/RegisterIME.cpp
  "$compiler" \
    -std=c++20 \
    -DUNICODE \
    -D_UNICODE \
    -DWIN32_LEAN_AND_MEAN \
    -DNOMINMAX \
    -D_WIN32_WINNT=0x0A00 \
    -Wall \
    -Wextra \
    -Werror \
    -Iplatforms/windows/native/src \
    -fsyntax-only \
    platforms/windows/native/src/WindowsPreferences.cpp \
    platforms/windows/native/src/WindowsSettings.cpp
done
