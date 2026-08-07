#include "SearchCandidateList.h"
#include "CandidateWindow.h"
#include "WindowsPreferences.h"
#include "SlimeIdentifiers.h"
#include "TsfCandidateCompat.h"

#include <windows.h>

#include <bit>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

namespace {

[[noreturn]] void CheckFailed(const char *expression, const char *file,
                              const int line) noexcept {
  std::fprintf(stderr, "%s:%d: check failed: %s\n", file, line, expression);
  std::abort();
}

#define CHECK(expression)                                                       \
  ((expression) ? static_cast<void>(0)                                         \
                : CheckFailed(#expression, __FILE__, __LINE__))

std::wstring CandidateValue(ITfCandidateString *candidate) {
  CHECK(candidate != nullptr);
  BSTR value = nullptr;
  CHECK(candidate->GetString(&value) == S_OK);
  CHECK(value != nullptr);
  std::wstring result(value, SysStringLen(value));
  SysFreeString(value);
  return result;
}

void TestSearchFiltering() {
  std::vector<std::wstring> candidates{L"北京大学", L"北京", L"京都",
                                        L"京都", L"大阪", L""};
  const auto filtered = FilterSearchCandidates(std::move(candidates), 2);
  CHECK((filtered == std::vector<std::wstring>{L"北京大学", L"京都"}));

  CHECK(FilterSearchCandidates({L"日本"}, 0).empty());
}

void TestCandidateList() {
  ITfCandidateList *list = nullptr;
  CHECK(CreateSearchCandidateList({L"日本", L"二本"}, &list) == S_OK);
  CHECK(list != nullptr);

  ULONG count = 0;
  CHECK(list->GetCandidateNum(&count) == S_OK);
  CHECK(count == 2);

  ITfCandidateString *first = nullptr;
  CHECK(list->GetCandidate(0, &first) == S_OK);
  CHECK(CandidateValue(first) == L"日本");
  ULONG index = 99;
  CHECK(first->GetIndex(&index) == S_OK);
  CHECK(index == 0);
  first->Release();

  IEnumTfCandidates *enumerator = nullptr;
  CHECK(list->EnumCandidates(&enumerator) == S_OK);
  ITfCandidateString *items[2]{};
  ULONG fetched = 0;
  CHECK(enumerator->Next(2, items, &fetched) == S_OK);
  CHECK(fetched == 2);
  CHECK(CandidateValue(items[0]) == L"日本");
  CHECK(CandidateValue(items[1]) == L"二本");
  items[0]->Release();
  items[1]->Release();
  CHECK(enumerator->Next(1, items, &fetched) == S_FALSE);
  CHECK(fetched == 0);
  enumerator->Release();

  CHECK(list->SetResult(1, CAND_SELECTED) == S_OK);
  CHECK(list->SetResult(2, CAND_FINALIZED) == E_INVALIDARG);
  list->Release();
  CHECK(SearchCandidateObjectCount() == 0);
}

void TestCandidateAutomationLifetime() {
  const long initialCount = CandidateAutomationObjectCount();
  {
    CandidateWindow window(GetModuleHandleW(nullptr), nullptr, nullptr);
    CHECK(CandidateAutomationObjectCount() == initialCount + 1);
  }
  CHECK(CandidateAutomationObjectCount() == initialCount);
}

void TestWindowsPreferencesParser() {
  WindowsPreferences parsed;
  CHECK(ParseWindowsPreferences(
      "[slime-settings-v1]\n"
      "live_conversion=0\n"
      "history_completion=1\n"
      "history_learning=0\n"
      "dictionary_packs=5\n"
      "date_format_mask=65\n",
      parsed));
  CHECK(!parsed.liveConversion);
  CHECK(parsed.historyCompletion);
  CHECK(!parsed.historyLearning);
  CHECK(parsed.dictionaryPacks == 5);
  CHECK(parsed.dateFormatMask == 65);

  WindowsPreferences defaults;
  CHECK(ParseWindowsPreferences("[slime-settings-v1]\n", defaults));
  CHECK(defaults == WindowsPreferences{});
  CHECK(!ParseWindowsPreferences(
      "[slime-settings-v1]\nlive_conversion=1\nlive_conversion=0\n",
      parsed));
  CHECK(!ParseWindowsPreferences(
      "[slime-settings-v1]\ndictionary_packs=8\n", parsed));
  CHECK(!ParseWindowsPreferences("live_conversion=1\n", parsed));

  const WindowsPreferences expected{false, true, false, 3, 9};
  CHECK(ParseWindowsPreferences(SerializeWindowsPreferences(expected), parsed));
  CHECK(parsed == expected);
}

void TestWindowsPreferencesAtomicSave() {
  wchar_t directory[MAX_PATH]{};
  CHECK(GetTempPathW(static_cast<DWORD>(std::size(directory)), directory) > 0);
  wchar_t path[MAX_PATH]{};
  CHECK(GetTempFileNameW(directory, L"slm", 0, path) != 0);

  const WindowsPreferences expected{false, false, true, 7, 0};
  CHECK(SaveWindowsPreferences(path, expected) == ERROR_SUCCESS);
  WindowsPreferences loaded;
  CHECK(LoadWindowsPreferences(path, loaded) ==
        WindowsPreferencesLoadStatus::loaded);
  CHECK(loaded == expected);
  CHECK(DeleteFileW(path) != FALSE);
}

void TestWindowsPreferencesMonitor() {
  wchar_t temporaryRoot[MAX_PATH]{};
  CHECK(GetTempPathW(static_cast<DWORD>(std::size(temporaryRoot)),
                     temporaryRoot) > 0);
  wchar_t uniquePath[MAX_PATH]{};
  CHECK(GetTempFileNameW(temporaryRoot, L"slm", 0, uniquePath) != 0);
  CHECK(DeleteFileW(uniquePath) != FALSE);
  CHECK(CreateDirectoryW(uniquePath, nullptr) != FALSE);
  const std::wstring settingsPath =
      std::wstring(uniquePath) + L"\\settings.ini";

  {
    WindowsPreferencesMonitor monitor;
    CHECK(monitor.Start(settingsPath));
    const WindowsPreferences preferences{false, true, true, 2, 3};
    CHECK(SaveWindowsPreferences(settingsPath, preferences) == ERROR_SUCCESS);
    bool observed = false;
    for (int attempt = 0; attempt < 100 && !observed; ++attempt) {
      observed = monitor.HasChanged();
      if (!observed) {
        Sleep(10);
      }
    }
    CHECK(observed);
  }
  CHECK(DeleteFileW(settingsPath.c_str()) != FALSE);
  CHECK(RemoveDirectoryW(uniquePath) != FALSE);
}

void TestTextServiceFunctionProvider() {
  const HRESULT initialize = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  CHECK(SUCCEEDED(initialize));
  HMODULE module = LoadLibraryW(L"SlimeIME.dll");
  CHECK(module != nullptr);

  using GetClassObjectFunction = HRESULT(__stdcall *)(REFCLSID, REFIID, void **);
  using CanUnloadFunction = HRESULT(__stdcall *)();
  const auto getClassObject = std::bit_cast<GetClassObjectFunction>(
      GetProcAddress(module, "DllGetClassObject"));
  const auto canUnload = std::bit_cast<CanUnloadFunction>(
      GetProcAddress(module, "DllCanUnloadNow"));
  CHECK(getClassObject != nullptr);
  CHECK(canUnload != nullptr);

  IClassFactory *factory = nullptr;
  CHECK(getClassObject(kTextServiceClsid, IID_IClassFactory,
                       reinterpret_cast<void **>(&factory)) == S_OK);
  CHECK(factory != nullptr);
  ITfFunctionProvider *functionProvider = nullptr;
  CHECK(factory->CreateInstance(
            nullptr, IID_ITfFunctionProvider,
            reinterpret_cast<void **>(&functionProvider)) == S_OK);
  CHECK(functionProvider != nullptr);

  IUnknown *configuration = nullptr;
  CHECK(functionProvider->GetFunction(GUID_NULL, IID_ITfFnConfigure,
                                      &configuration) == S_OK);
  CHECK(configuration != nullptr);
  ITfFnConfigure *configure = nullptr;
  CHECK(configuration->QueryInterface(
            IID_ITfFnConfigure,
            reinterpret_cast<void **>(&configure)) == S_OK);
  BSTR configureName = nullptr;
  CHECK(configure->GetDisplayName(&configureName) == S_OK);
  CHECK(std::wstring(configureName, SysStringLen(configureName)) ==
        L"Slime settings");
  SysFreeString(configureName);
  configure->Release();
  configuration->Release();

  IUnknown *search = nullptr;
  CHECK(functionProvider->GetFunction(GUID_NULL,
                                      IID_ITfFnSearchCandidateProvider,
                                      &search) == S_OK);
  CHECK(search != nullptr);
  search->Release();
  functionProvider->Release();
  factory->Release();
  CHECK(canUnload() == S_OK);
  CHECK(FreeLibrary(module) != FALSE);
  CoUninitialize();
}

} // namespace

int wmain() {
  TestSearchFiltering();
  TestCandidateList();
  TestCandidateAutomationLifetime();
  TestWindowsPreferencesParser();
  TestWindowsPreferencesAtomicSave();
  TestWindowsPreferencesMonitor();
  TestTextServiceFunctionProvider();
  return 0;
}
