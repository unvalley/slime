#pragma once

#include <windows.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <string>
#include <string_view>

inline constexpr std::uint32_t kWindowsDictionaryPackMask = 0x07;
inline constexpr std::uint32_t kWindowsAllDateFormatMask = 0x7f;

struct WindowsPreferences {
  bool liveConversion = true;
  bool typoCorrectionEnabled = false;
  bool historyCompletion = true;
  bool historyLearning = true;
  std::uint32_t dictionaryPacks = 0;
  std::uint32_t dateFormatMask = kWindowsAllDateFormatMask;

  bool operator==(const WindowsPreferences &) const = default;
};

enum class WindowsPreferencesLoadStatus {
  loaded,
  notFound,
  invalid,
  ioError,
};

std::wstring WindowsPreferencesPath() noexcept;
bool ParseWindowsPreferences(std::string_view contents,
                             WindowsPreferences &preferences) noexcept;
std::string SerializeWindowsPreferences(
    const WindowsPreferences &preferences);
WindowsPreferencesLoadStatus LoadWindowsPreferences(
    const std::wstring &path, WindowsPreferences &preferences) noexcept;
DWORD SaveWindowsPreferences(const std::wstring &path,
                             const WindowsPreferences &preferences) noexcept;

class WindowsPreferencesMonitor final {
public:
  WindowsPreferencesMonitor() noexcept = default;
  ~WindowsPreferencesMonitor();

  WindowsPreferencesMonitor(const WindowsPreferencesMonitor &) = delete;
  WindowsPreferencesMonitor &operator=(const WindowsPreferencesMonitor &) =
      delete;

  bool Start(const std::wstring &preferencesPath) noexcept;
  bool HasChanged() noexcept;

private:
  bool Arm() noexcept;
  void Stop() noexcept;

  HANDLE directory_ = INVALID_HANDLE_VALUE;
  HANDLE event_ = nullptr;
  OVERLAPPED overlapped_{};
  alignas(DWORD) std::array<std::byte, 4096> buffer_{};
};
