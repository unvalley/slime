#include "WindowsPreferences.h"

#include <algorithm>
#include <charconv>
#include <limits>
#include <string>
#include <system_error>
#include <vector>

namespace {

constexpr std::string_view kSettingsHeader = "[slime-settings-v1]";
constexpr std::size_t kMaximumSettingsBytes = 64 * 1024;

bool ParseBool(const std::string_view value, bool &parsed) noexcept {
  if (value == "0") {
    parsed = false;
    return true;
  }
  if (value == "1") {
    parsed = true;
    return true;
  }
  return false;
}

bool ParseMask(const std::string_view value, const std::uint32_t allowed,
               std::uint32_t &parsed) noexcept {
  std::uint32_t number = 0;
  const auto result =
      std::from_chars(value.data(), value.data() + value.size(), number);
  if (result.ec != std::errc{} || result.ptr != value.data() + value.size() ||
      (number & ~allowed) != 0) {
    return false;
  }
  parsed = number;
  return true;
}

bool EnsureParentDirectory(const std::wstring &path) noexcept {
  try {
    const std::size_t separator = path.find_last_of(L"\\/");
    if (separator == std::wstring::npos || separator == 0) {
      SetLastError(ERROR_PATH_NOT_FOUND);
      return false;
    }
    const std::wstring directory = path.substr(0, separator);
    return CreateDirectoryW(directory.c_str(), nullptr) != FALSE ||
           GetLastError() == ERROR_ALREADY_EXISTS;
  } catch (...) {
    SetLastError(ERROR_OUTOFMEMORY);
    return false;
  }
}

} // namespace

std::wstring WindowsPreferencesPath() noexcept {
  try {
    const DWORD required = GetEnvironmentVariableW(L"LOCALAPPDATA", nullptr, 0);
    if (required == 0) {
      return {};
    }
    std::vector<wchar_t> buffer(required);
    const DWORD written = GetEnvironmentVariableW(
        L"LOCALAPPDATA", buffer.data(), static_cast<DWORD>(buffer.size()));
    if (written == 0 || written >= buffer.size()) {
      return {};
    }
    std::wstring path(buffer.data(), written);
    path.append(L"\\Slime\\settings.ini");
    return path;
  } catch (...) {
    return {};
  }
}

bool ParseWindowsPreferences(const std::string_view contents,
                             WindowsPreferences &preferences) noexcept {
  try {
    WindowsPreferences parsed;
    std::uint32_t seen = 0;
    std::size_t position = 0;
    bool headerSeen = false;
    while (position <= contents.size()) {
      const std::size_t newline = contents.find('\n', position);
      const std::size_t end =
          newline == std::string_view::npos ? contents.size() : newline;
      std::string_view line = contents.substr(position, end - position);
      if (!line.empty() && line.back() == '\r') {
        line.remove_suffix(1);
      }
      if (!headerSeen) {
        if (line != kSettingsHeader) {
          return false;
        }
        headerSeen = true;
      } else if (!line.empty()) {
        const std::size_t equals = line.find('=');
        if (equals == std::string_view::npos || equals == 0 ||
            equals + 1 >= line.size()) {
          return false;
        }
        const std::string_view key = line.substr(0, equals);
        const std::string_view value = line.substr(equals + 1);
        std::uint32_t bit = 0;
        bool valid = false;
        if (key == "live_conversion") {
          bit = 1U << 0;
          valid = ParseBool(value, parsed.liveConversion);
        } else if (key == "history_completion") {
          bit = 1U << 1;
          valid = ParseBool(value, parsed.historyCompletion);
        } else if (key == "history_learning") {
          bit = 1U << 2;
          valid = ParseBool(value, parsed.historyLearning);
        } else if (key == "dictionary_packs") {
          bit = 1U << 3;
          valid = ParseMask(value, kWindowsDictionaryPackMask,
                            parsed.dictionaryPacks);
        } else if (key == "date_format_mask") {
          bit = 1U << 4;
          valid = ParseMask(value, kWindowsAllDateFormatMask,
                            parsed.dateFormatMask);
        } else {
          // Unknown keys are retained as a forward-compatible extension point.
          valid = true;
        }
        if (!valid || (bit != 0 && (seen & bit) != 0)) {
          return false;
        }
        seen |= bit;
      }
      if (newline == std::string_view::npos) {
        break;
      }
      position = newline + 1;
    }
    if (!headerSeen) {
      return false;
    }
    preferences = parsed;
    return true;
  } catch (...) {
    return false;
  }
}

std::string SerializeWindowsPreferences(
    const WindowsPreferences &preferences) {
  std::string result;
  result.reserve(160);
  result.append(kSettingsHeader);
  result.append("\r\nlive_conversion=");
  result.push_back(preferences.liveConversion ? '1' : '0');
  result.append("\r\nhistory_completion=");
  result.push_back(preferences.historyCompletion ? '1' : '0');
  result.append("\r\nhistory_learning=");
  result.push_back(preferences.historyLearning ? '1' : '0');
  result.append("\r\ndictionary_packs=");
  result.append(std::to_string(preferences.dictionaryPacks &
                               kWindowsDictionaryPackMask));
  result.append("\r\ndate_format_mask=");
  result.append(std::to_string(preferences.dateFormatMask &
                               kWindowsAllDateFormatMask));
  result.append("\r\n");
  return result;
}

WindowsPreferencesLoadStatus LoadWindowsPreferences(
    const std::wstring &path, WindowsPreferences &preferences) noexcept {
  if (path.empty()) {
    return WindowsPreferencesLoadStatus::ioError;
  }
  HANDLE file = CreateFileW(path.c_str(), GENERIC_READ,
                            FILE_SHARE_READ | FILE_SHARE_WRITE |
                                FILE_SHARE_DELETE,
                            nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL,
                            nullptr);
  if (file == INVALID_HANDLE_VALUE) {
    const DWORD error = GetLastError();
    return error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND
               ? WindowsPreferencesLoadStatus::notFound
               : WindowsPreferencesLoadStatus::ioError;
  }
  LARGE_INTEGER size{};
  if (!GetFileSizeEx(file, &size) || size.QuadPart < 0 ||
      size.QuadPart > static_cast<LONGLONG>(kMaximumSettingsBytes)) {
    CloseHandle(file);
    return WindowsPreferencesLoadStatus::invalid;
  }
  try {
    std::string contents(static_cast<std::size_t>(size.QuadPart), '\0');
    DWORD total = 0;
    while (total < contents.size()) {
      DWORD read = 0;
      const DWORD remaining = static_cast<DWORD>(std::min<std::size_t>(
          contents.size() - total, std::numeric_limits<DWORD>::max()));
      if (!ReadFile(file, contents.data() + total, remaining, &read, nullptr) ||
          read == 0) {
        CloseHandle(file);
        return WindowsPreferencesLoadStatus::ioError;
      }
      total += read;
    }
    CloseHandle(file);
    WindowsPreferences parsed;
    if (!ParseWindowsPreferences(contents, parsed)) {
      return WindowsPreferencesLoadStatus::invalid;
    }
    preferences = parsed;
    return WindowsPreferencesLoadStatus::loaded;
  } catch (...) {
    CloseHandle(file);
    return WindowsPreferencesLoadStatus::ioError;
  }
}

DWORD SaveWindowsPreferences(const std::wstring &path,
                             const WindowsPreferences &preferences) noexcept {
  if (path.empty() || !EnsureParentDirectory(path)) {
    const DWORD error = GetLastError();
    return error == ERROR_SUCCESS ? ERROR_PATH_NOT_FOUND : error;
  }
  try {
    const std::string contents = SerializeWindowsPreferences(preferences);
    std::wstring temporary = path;
    temporary.append(L".tmp-");
    temporary.append(std::to_wstring(GetCurrentProcessId()));
    temporary.push_back(L'-');
    temporary.append(std::to_wstring(GetTickCount64()));
    HANDLE file = CreateFileW(temporary.c_str(), GENERIC_WRITE, 0, nullptr,
                              CREATE_NEW,
                              FILE_ATTRIBUTE_NORMAL | FILE_FLAG_WRITE_THROUGH,
                              nullptr);
    if (file == INVALID_HANDLE_VALUE) {
      return GetLastError();
    }
    DWORD total = 0;
    while (total < contents.size()) {
      DWORD written = 0;
      const DWORD remaining = static_cast<DWORD>(std::min<std::size_t>(
          contents.size() - total, std::numeric_limits<DWORD>::max()));
      if (!WriteFile(file, contents.data() + total, remaining, &written,
                     nullptr) ||
          written == 0) {
        const DWORD error = GetLastError();
        CloseHandle(file);
        DeleteFileW(temporary.c_str());
        return error;
      }
      total += written;
    }
    if (!FlushFileBuffers(file)) {
      const DWORD error = GetLastError();
      CloseHandle(file);
      DeleteFileW(temporary.c_str());
      return error;
    }
    CloseHandle(file);
    if (!MoveFileExW(temporary.c_str(), path.c_str(),
                     MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)) {
      const DWORD error = GetLastError();
      DeleteFileW(temporary.c_str());
      return error;
    }
    return ERROR_SUCCESS;
  } catch (...) {
    return ERROR_OUTOFMEMORY;
  }
}

WindowsPreferencesMonitor::~WindowsPreferencesMonitor() { Stop(); }

bool WindowsPreferencesMonitor::Start(
    const std::wstring &preferencesPath) noexcept {
  Stop();
  try {
    const std::size_t separator = preferencesPath.find_last_of(L"\\/");
    if (separator == std::wstring::npos || separator == 0) {
      return false;
    }
    const std::wstring directory = preferencesPath.substr(0, separator);
    directory_ = CreateFileW(
        directory.c_str(), FILE_LIST_DIRECTORY,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
        OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
        nullptr);
    if (directory_ == INVALID_HANDLE_VALUE) {
      return false;
    }
    event_ = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    if (event_ == nullptr || !Arm()) {
      Stop();
      return false;
    }
    return true;
  } catch (...) {
    Stop();
    return false;
  }
}

bool WindowsPreferencesMonitor::HasChanged() noexcept {
  if (directory_ == INVALID_HANDLE_VALUE || event_ == nullptr ||
      WaitForSingleObject(event_, 0) != WAIT_OBJECT_0) {
    return false;
  }
  DWORD transferred = 0;
  bool changed = false;
  if (GetOverlappedResult(directory_, &overlapped_, &transferred, FALSE)) {
    if (transferred == 0) {
      changed = true;
    } else {
      std::size_t offset = 0;
      constexpr std::size_t headerSize =
          offsetof(FILE_NOTIFY_INFORMATION, FileName);
      while (offset + headerSize <= transferred) {
        const auto *notification =
            reinterpret_cast<const FILE_NOTIFY_INFORMATION *>(buffer_.data() +
                                                               offset);
        const std::size_t nameBytes = notification->FileNameLength;
        if (nameBytes % sizeof(wchar_t) != 0 ||
            offset + headerSize + nameBytes > transferred) {
          changed = true;
          break;
        }
        const int nameLength = static_cast<int>(nameBytes / sizeof(wchar_t));
        if (CompareStringOrdinal(notification->FileName, nameLength,
                                 L"settings.ini", -1, TRUE) == CSTR_EQUAL) {
          changed = true;
        }
        if (notification->NextEntryOffset == 0) {
          break;
        }
        if (notification->NextEntryOffset < headerSize ||
            offset + notification->NextEntryOffset > transferred) {
          changed = true;
          break;
        }
        offset += notification->NextEntryOffset;
      }
    }
  } else if (GetLastError() != ERROR_IO_INCOMPLETE) {
    changed = true;
  }
  if (!Arm()) {
    Stop();
  }
  return changed;
}

bool WindowsPreferencesMonitor::Arm() noexcept {
  if (directory_ == INVALID_HANDLE_VALUE || event_ == nullptr) {
    return false;
  }
  ResetEvent(event_);
  overlapped_ = {};
  overlapped_.hEvent = event_;
  const BOOL result = ReadDirectoryChangesW(
      directory_, buffer_.data(), static_cast<DWORD>(buffer_.size()), FALSE,
      FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE |
          FILE_NOTIFY_CHANGE_SIZE,
      nullptr, &overlapped_, nullptr);
  return result != FALSE || GetLastError() == ERROR_IO_PENDING;
}

void WindowsPreferencesMonitor::Stop() noexcept {
  if (directory_ != INVALID_HANDLE_VALUE) {
    CancelIoEx(directory_, &overlapped_);
    CloseHandle(directory_);
    directory_ = INVALID_HANDLE_VALUE;
  }
  if (event_ != nullptr) {
    CloseHandle(event_);
    event_ = nullptr;
  }
  overlapped_ = {};
}
