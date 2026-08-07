#include <windows.h>
#include <shlwapi.h>

#include <cstring>
#include <iostream>
#include <limits>
#include <string>
#include <vector>

#include "SlimeIdentifiers.h"

namespace {

using DllRegistrationFunction = HRESULT(__stdcall *)();
using InstallLayoutOrTipFunction = BOOL(WINAPI *)(LPCWSTR, DWORD);

constexpr DWORD kInstallLayoutOrTipUninstall = 0x00000001;

template <typename Function>
Function LoadFunction(HMODULE module, const char *name) noexcept {
  static_assert(sizeof(Function) == sizeof(FARPROC));
  const FARPROC address = GetProcAddress(module, name);
  Function function = nullptr;
  std::memcpy(&function, &address, sizeof(function));
  return function;
}

int PrintWindowsError(const wchar_t *operation) {
  std::wcerr << operation << L" failed with Win32 error " << GetLastError() << L'\n';
  return 1;
}

int PrintHresult(const wchar_t *operation, const HRESULT result) {
  std::wcerr << operation << L" failed with HRESULT 0x" << std::hex
             << static_cast<unsigned long>(result) << L'\n';
  return 1;
}

bool ResolveAbsolutePath(const wchar_t *input, std::wstring &output) {
  const DWORD required = GetFullPathNameW(input, 0, nullptr, nullptr);
  if (required == 0 || required > static_cast<DWORD>(std::numeric_limits<int>::max())) {
    return false;
  }
  std::vector<wchar_t> buffer(required);
  const DWORD length = GetFullPathNameW(input, required, buffer.data(), nullptr);
  if (length == 0 || length >= required) {
    return false;
  }
  output.assign(buffer.data(), length);
  return PathIsRelativeW(output.c_str()) == FALSE;
}

bool EnableProfile(const bool uninstall) {
  HMODULE input = LoadLibraryExW(L"input.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
  if (input == nullptr) {
    return false;
  }
  const auto installLayoutOrTip =
      LoadFunction<InstallLayoutOrTipFunction>(input, "InstallLayoutOrTip");
  const BOOL result = installLayoutOrTip == nullptr
                          ? FALSE
                          : installLayoutOrTip(kProfileList,
                                               uninstall ? kInstallLayoutOrTipUninstall : 0);
  FreeLibrary(input);
  return result != FALSE;
}

} // namespace

int wmain(const int argumentCount, wchar_t **arguments) {
  if (argumentCount != 3 ||
      (std::wstring(arguments[1]) != L"install" &&
       std::wstring(arguments[1]) != L"uninstall")) {
    std::wcerr << L"usage: SlimeIMERegister <install|uninstall> <absolute SlimeIME.dll path>\n";
    return 2;
  }

  const bool uninstall = std::wstring(arguments[1]) == L"uninstall";
  std::wstring dllPath;
  if (!ResolveAbsolutePath(arguments[2], dllPath)) {
    return PrintWindowsError(L"GetFullPathNameW");
  }
  HMODULE ime = LoadLibraryExW(dllPath.c_str(), nullptr, LOAD_WITH_ALTERED_SEARCH_PATH);
  if (ime == nullptr) {
    return PrintWindowsError(L"LoadLibraryExW(SlimeIME.dll)");
  }

  const char *entryPoint = uninstall ? "DllUnregisterServer" : "DllRegisterServer";
  const auto registration = LoadFunction<DllRegistrationFunction>(ime, entryPoint);
  if (registration == nullptr) {
    FreeLibrary(ime);
    return PrintWindowsError(L"GetProcAddress");
  }

  HRESULT result = S_OK;
  if (uninstall) {
    EnableProfile(true);
    result = registration();
  } else {
    result = registration();
    if (SUCCEEDED(result) && !EnableProfile(false)) {
      const DWORD error = GetLastError();
      result = error == ERROR_SUCCESS ? E_FAIL : HRESULT_FROM_WIN32(error);
      const auto rollback =
          LoadFunction<DllRegistrationFunction>(ime, "DllUnregisterServer");
      if (rollback != nullptr) {
        rollback();
      }
    }
  }
  FreeLibrary(ime);
  if (FAILED(result)) {
    return PrintHresult(uninstall ? L"uninstall" : L"install", result);
  }
  return 0;
}
