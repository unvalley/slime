#pragma once

#include <windows.h>

// These identifiers are part of Slime's Windows installation identity. Never
// regenerate them after a public build: upgrades and uninstall depend on them.

// {E4F851DD-9801-4582-B84F-7D76B7EEC049}
inline constexpr CLSID kTextServiceClsid = {
    0xe4f851dd,
    0x9801,
    0x4582,
    {0xb8, 0x4f, 0x7d, 0x76, 0xb7, 0xee, 0xc0, 0x49}};

// {C2B62953-18E5-4DFB-93AD-D407017A9E99}
inline constexpr GUID kLanguageProfileGuid = {
    0xc2b62953,
    0x18e5,
    0x4dfb,
    {0x93, 0xad, 0xd4, 0x07, 0x01, 0x7a, 0x9e, 0x99}};

inline constexpr LANGID kJapaneseLanguage =
    MAKELANGID(LANG_JAPANESE, SUBLANG_JAPANESE_JAPAN);
inline constexpr wchar_t kDescription[] = L"Slime Japanese IME";
inline constexpr wchar_t kProfileList[] =
    L"0x0411:{E4F851DD-9801-4582-B84F-7D76B7EEC049}"
    L"{C2B62953-18E5-4DFB-93AD-D407017A9E99}";
