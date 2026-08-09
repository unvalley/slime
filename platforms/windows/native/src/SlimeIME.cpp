#include <windows.h>
#include <ctfutb.h>
#include <msctf.h>
#include <wrl/client.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cstdint>
#include <iterator>
#include <limits>
#include <memory>
#include <new>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "slime_ffi.h"
#include "CandidateWindow.h"
#include "SearchCandidateList.h"
#include "SlimeIdentifiers.h"
#include "TsfCandidateCompat.h"
#include "WindowsPreferences.h"

using Microsoft::WRL::ComPtr;

namespace {

std::atomic_long g_objectCount = 0;
std::atomic_long g_serverLocks = 0;
HMODULE g_module = nullptr;
constexpr TfClientId kInvalidClientId = 0;
constexpr std::size_t kSearchCandidateLimit = 20;
constexpr LONG kDocumentContextUtf16Limit = 256;
constexpr GUID kImmersiveSupportCategory = {
    0x13a016df,
    0x560b,
    0x46cd,
    {0x94, 0x7a, 0x4c, 0x3a, 0xf1, 0xe0, 0xe3, 0x5d}};
constexpr GUID kSystraySupportCategory = {
    0x25504fb4,
    0x7bab,
    0x4bc1,
    {0x9c, 0x69, 0xcf, 0x81, 0x89, 0x0f, 0x0e, 0xf5}};
constexpr GUID kUiElementEnabledCategory = {
    0x49d2f9cf,
    0x1f5e,
    0x11d7,
    {0xa6, 0xd3, 0x00, 0x06, 0x5b, 0x84, 0x43, 0x5c}};

class TextService;

constexpr GUID kCandidateUiGuid = {
    0x09b8bdce,
    0x7b7d,
    0x4fd3,
    {0x99, 0xcf, 0xc1, 0x40, 0x23, 0x68, 0x95, 0x6f}};
constexpr GUID kSearchBoxIntegrationStyle = {
    0xe6d1bd11,
    0x82f7,
    0x4903,
    {0xae, 0x21, 0x1a, 0x63, 0x97, 0xcd, 0xe2, 0xeb}};

std::wstring Utf8ToWide(SlimeStringView value);

CandidatePresentation MakeCandidatePresentation(
    const SlimeCandidateViewV2 &candidate) {
  std::wstring value = Utf8ToWide(candidate.value);
  if (value.empty()) {
    value = Utf8ToWide(candidate.display);
  }
  return BuildCandidatePresentation(std::move(value), candidate.annotation,
                                    Utf8ToWide(candidate.detail));
}

class CandidateUiElement final : public ITfCandidateListUIElementBehavior,
                                 public ITfIntegratableCandidateListUIElement {
public:
  using ShowCallback = void (*)(void *context, bool show) noexcept;
  using SelectionCallback = bool (*)(void *context, UINT candidateIndex,
                                     bool accept) noexcept;
  using KeyCallback = bool (*)(void *context, WPARAM virtualKey,
                               LPARAM keyData) noexcept;
  using AbortCallback = bool (*)(void *context) noexcept;

  CandidateUiElement(ITfDocumentMgr *documentManager,
                     std::vector<CandidatePresentation> candidates,
                     const std::size_t selected, void *callbackContext,
                     ShowCallback showCallback,
                     SelectionCallback selectionCallback,
                     KeyCallback keyCallback, AbortCallback abortCallback)
      : documentManager_(documentManager), candidates_(std::move(candidates)),
        callbackContext_(callbackContext), showCallback_(showCallback),
        selectionCallback_(selectionCallback), keyCallback_(keyCallback),
        abortCallback_(abortCallback) {
    ++g_objectCount;
    UpdateSelection(selected);
    ResetPages();
  }

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **object) override {
    if (object == nullptr) {
      return E_POINTER;
    }
    *object = nullptr;
    if (iid == IID_IUnknown || iid == IID_ITfUIElement ||
        iid == IID_ITfCandidateListUIElement) {
      *object = static_cast<ITfCandidateListUIElementBehavior *>(this);
      AddRef();
      return S_OK;
    }
    if (iid == IID_ITfCandidateListUIElementBehavior) {
      *object = static_cast<ITfCandidateListUIElementBehavior *>(this);
      AddRef();
      return S_OK;
    }
    if (iid == IID_ITfIntegratableCandidateListUIElement) {
      *object = static_cast<ITfIntegratableCandidateListUIElement *>(this);
      AddRef();
      return S_OK;
    }
    return E_NOINTERFACE;
  }

  ULONG STDMETHODCALLTYPE AddRef() override { return ++referenceCount_; }

  ULONG STDMETHODCALLTYPE Release() override {
    const ULONG remaining = --referenceCount_;
    if (remaining == 0) {
      delete this;
    }
    return remaining;
  }

  HRESULT STDMETHODCALLTYPE GetDescription(BSTR *description) override {
    if (description == nullptr) {
      return E_POINTER;
    }
    *description = SysAllocString(L"Slime candidates");
    return *description != nullptr ? S_OK : E_OUTOFMEMORY;
  }

  HRESULT STDMETHODCALLTYPE GetGUID(GUID *guid) override {
    if (guid == nullptr) {
      return E_POINTER;
    }
    *guid = kCandidateUiGuid;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE Show(const BOOL show) override {
    shown_ = show;
    if (showCallback_ != nullptr) {
      showCallback_(callbackContext_, show != FALSE);
    }
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE IsShown(BOOL *shown) override {
    if (shown == nullptr) {
      return E_POINTER;
    }
    *shown = shown_;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE GetUpdatedFlags(DWORD *flags) override {
    if (flags == nullptr) {
      return E_POINTER;
    }
    *flags = updatedFlags_;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE GetDocumentMgr(ITfDocumentMgr **documentManager) override {
    if (documentManager == nullptr) {
      return E_POINTER;
    }
    *documentManager = documentManager_.Get();
    if (*documentManager == nullptr) {
      return E_FAIL;
    }
    (*documentManager)->AddRef();
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE GetCount(UINT *count) override {
    if (count == nullptr) {
      return E_POINTER;
    }
    if (candidates_.size() > std::numeric_limits<UINT>::max()) {
      return E_FAIL;
    }
    *count = static_cast<UINT>(candidates_.size());
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE GetSelection(UINT *index) override {
    if (index == nullptr) {
      return E_POINTER;
    }
    *index = selected_;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE GetString(const UINT index, BSTR *value) override {
    if (value == nullptr) {
      return E_POINTER;
    }
    *value = nullptr;
    if (index >= candidates_.size() ||
        candidates_[index].accessibleName.size() >
            std::numeric_limits<UINT>::max()) {
      return E_INVALIDARG;
    }
    *value = SysAllocStringLen(
        candidates_[index].accessibleName.data(),
        static_cast<UINT>(candidates_[index].accessibleName.size()));
    return *value != nullptr ? S_OK : E_OUTOFMEMORY;
  }

  HRESULT STDMETHODCALLTYPE GetPageIndex(UINT *indexes, const UINT size,
                                         UINT *pageCount) override {
    if (pageCount == nullptr || (size > 0 && indexes == nullptr) ||
        pageStarts_.size() > std::numeric_limits<UINT>::max()) {
      return E_INVALIDARG;
    }
    *pageCount = static_cast<UINT>(pageStarts_.size());
    const UINT copyCount = std::min(size, *pageCount);
    if (copyCount > 0) {
      std::copy_n(pageStarts_.begin(), copyCount, indexes);
    }
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE SetPageIndex(UINT *indexes, const UINT pageCount) override {
    if (pageCount == 0 || indexes == nullptr || indexes[0] != 0) {
      return E_INVALIDARG;
    }
    try {
      std::vector<UINT> pages(indexes, indexes + pageCount);
      if (!std::is_sorted(pages.begin(), pages.end()) ||
          std::adjacent_find(pages.begin(), pages.end()) != pages.end() ||
          pages.back() >= candidates_.size()) {
        return E_INVALIDARG;
      }
      pageStarts_ = std::move(pages);
      updatedFlags_ = TF_CLUIE_PAGEINDEX | TF_CLUIE_CURRENTPAGE;
      return S_OK;
    } catch (...) {
      return E_OUTOFMEMORY;
    }
  }

  HRESULT STDMETHODCALLTYPE GetCurrentPage(UINT *page) override {
    if (page == nullptr) {
      return E_POINTER;
    }
    const auto next = std::upper_bound(pageStarts_.begin(), pageStarts_.end(), selected_);
    *page = next == pageStarts_.begin()
                ? 0
                : static_cast<UINT>(std::distance(pageStarts_.begin(), next) - 1);
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE SetSelection(const UINT index) override {
    if (index >= candidates_.size()) {
      return E_INVALIDARG;
    }
    return selectionCallback_ != nullptr &&
                   selectionCallback_(callbackContext_, index, false)
               ? S_OK
               : E_FAIL;
  }

  HRESULT STDMETHODCALLTYPE Finalize() override {
    if (candidates_.empty()) {
      return E_FAIL;
    }
    return selectionCallback_ != nullptr &&
                   selectionCallback_(callbackContext_, selected_, true)
               ? S_OK
               : E_FAIL;
  }

  HRESULT STDMETHODCALLTYPE Abort() override {
    return abortCallback_ != nullptr && abortCallback_(callbackContext_)
               ? S_OK
               : E_FAIL;
  }

  HRESULT STDMETHODCALLTYPE SetIntegrationStyle(const GUID style) override {
    return IsEqualGUID(style, kSearchBoxIntegrationStyle) ? S_OK : E_NOTIMPL;
  }

  HRESULT STDMETHODCALLTYPE GetSelectionStyle(
      TfIntegratableCandidateListSelectionStyle *style) override {
    if (style == nullptr) {
      return E_POINTER;
    }
    *style = STYLE_ACTIVE_SELECTION;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE OnKeyDown(const WPARAM virtualKey,
                                      const LPARAM keyData,
                                      BOOL *eaten) override {
    if (eaten == nullptr) {
      return E_POINTER;
    }
    *eaten = keyCallback_ != nullptr &&
                     keyCallback_(callbackContext_, virtualKey, keyData)
                 ? TRUE
                 : FALSE;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE ShowCandidateNumbers(BOOL *show) override {
    if (show == nullptr) {
      return E_POINTER;
    }
    *show = TRUE;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE FinalizeExactCompositionString() override {
    return Finalize();
  }

  void Update(std::vector<CandidatePresentation> candidates,
              const std::size_t selected) {
    candidates_ = std::move(candidates);
    UpdateSelection(selected);
    ResetPages();
    updatedFlags_ = TF_CLUIE_COUNT | TF_CLUIE_SELECTION | TF_CLUIE_STRING |
                    TF_CLUIE_PAGEINDEX | TF_CLUIE_CURRENTPAGE;
  }

  [[nodiscard]] const std::vector<CandidatePresentation> &candidates()
      const noexcept {
    return candidates_;
  }

  [[nodiscard]] std::size_t selected() const noexcept { return selected_; }

private:
  ~CandidateUiElement() { --g_objectCount; }

  void UpdateSelection(const std::size_t selected) noexcept {
    selected_ = selected < candidates_.size() ? static_cast<UINT>(selected) : 0;
  }

  void ResetPages() {
    pageStarts_.clear();
    for (std::size_t index = 0; index < candidates_.size();
         index += kSlimeCandidatePageSize) {
      pageStarts_.push_back(static_cast<UINT>(index));
    }
  }

  std::atomic_ulong referenceCount_{1};
  ComPtr<ITfDocumentMgr> documentManager_;
  std::vector<CandidatePresentation> candidates_;
  std::vector<UINT> pageStarts_;
  UINT selected_ = 0;
  DWORD updatedFlags_ = TF_CLUIE_DOCUMENTMGR | TF_CLUIE_COUNT | TF_CLUIE_SELECTION |
                        TF_CLUIE_STRING | TF_CLUIE_PAGEINDEX | TF_CLUIE_CURRENTPAGE;
  void *callbackContext_ = nullptr;
  ShowCallback showCallback_ = nullptr;
  SelectionCallback selectionCallback_ = nullptr;
  KeyCallback keyCallback_ = nullptr;
  AbortCallback abortCallback_ = nullptr;
  BOOL shown_ = FALSE;
};

struct EngineAction {
  std::uint32_t kind = SLIME_ACTION_FORWARD_KEY;
  std::wstring text;
  std::vector<CandidatePresentation> candidates;
  std::size_t selected = std::numeric_limits<std::size_t>::max();
  std::size_t selectionStart = std::numeric_limits<std::size_t>::max();
  std::size_t selectionLength = 0;
};

struct EngineActionCollection {
  std::vector<EngineAction> actions;
  bool failed = false;
};

std::wstring Utf8ToWide(const SlimeStringView value) {
  if (value.data == nullptr || value.len == 0) {
    return {};
  }
  if (value.len > static_cast<std::size_t>(std::numeric_limits<int>::max())) {
    return {};
  }
  const auto length = static_cast<int>(value.len);
  const int wideLength = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS,
                                             reinterpret_cast<const char *>(value.data), length,
                                             nullptr, 0);
  if (wideLength <= 0) {
    return {};
  }
  std::wstring result(static_cast<std::size_t>(wideLength), L'\0');
  if (MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS,
                          reinterpret_cast<const char *>(value.data), length, result.data(),
                          wideLength) != wideLength) {
    return {};
  }
  return result;
}

std::optional<std::string> WideToUtf8(const wchar_t *value,
                                      const std::size_t length) {
  if (length == 0) {
    return std::string{};
  }
  if (value == nullptr ||
      length > static_cast<std::size_t>(std::numeric_limits<int>::max())) {
    return std::nullopt;
  }
  const int inputLength = static_cast<int>(length);
  const int utf8Length = WideCharToMultiByte(
      CP_UTF8, WC_ERR_INVALID_CHARS, value, inputLength, nullptr, 0, nullptr,
      nullptr);
  if (utf8Length <= 0) {
    return std::nullopt;
  }
  std::string result(static_cast<std::size_t>(utf8Length), '\0');
  if (WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value, inputLength,
                          result.data(), utf8Length, nullptr,
                          nullptr) != utf8Length) {
    return std::nullopt;
  }
  return result;
}

struct EngineStringCollection {
  std::vector<std::wstring> values;
  bool failed = false;
};

void CollectString(void *context, const SlimeStringView value) noexcept {
  if (context == nullptr) {
    return;
  }
  auto &collection = *static_cast<EngineStringCollection *>(context);
  if (collection.failed) {
    return;
  }
  try {
    collection.values.push_back(Utf8ToWide(value));
  } catch (...) {
    collection.failed = true;
  }
}

void CollectActionV2(void *context, const SlimeActionViewV2 *view) noexcept {
  if (context == nullptr || view == nullptr) {
    return;
  }
  auto &collection = *static_cast<EngineActionCollection *>(context);
  if (collection.failed) {
    return;
  }
  try {
    EngineAction action;
    action.kind = view->kind;
    action.text = Utf8ToWide(view->text);
    action.selected = view->selected;
    action.selectionStart = view->selection_start;
    action.selectionLength = view->selection_length;
    if (view->candidates != nullptr) {
      action.candidates.reserve(view->candidate_count);
      for (std::size_t index = 0; index < view->candidate_count; ++index) {
        action.candidates.push_back(
            MakeCandidatePresentation(view->candidates[index]));
      }
    }
    collection.actions.push_back(std::move(action));
  } catch (...) {
    collection.failed = true;
  }
}

void IgnoreActionV2(void *, const SlimeActionViewV2 *) noexcept {}

void ApplyWindowsPreferences(SlimeHandle *engine,
                             const WindowsPreferences &preferences,
                             const bool privateMode) noexcept {
  if (engine == nullptr) {
    return;
  }
  SlimeBuffer response = slime_set_options_v5(
      engine, preferences.liveConversion, preferences.historyCompletion,
      preferences.historyLearning, preferences.dictionaryPacks, privateMode,
      preferences.dateFormatMask);
  slime_buffer_destroy(response);
}

SlimeHandle *CreateEngine(const std::wstring &preferencesPath,
                          WindowsPreferences &preferences) noexcept {
  if (!preferencesPath.empty()) {
    WindowsPreferences loaded;
    if (LoadWindowsPreferences(preferencesPath, loaded) ==
        WindowsPreferencesLoadStatus::loaded) {
      preferences = loaded;
    }
  }
  SlimeHandle *engine = nullptr;
  try {
    const std::size_t separator = preferencesPath.find_last_of(L"\\/");
    if (separator != std::wstring::npos && separator > 0) {
      const std::wstring dataDirectory = preferencesPath.substr(0, separator);
      if (CreateDirectoryW(dataDirectory.c_str(), nullptr) ||
          GetLastError() == ERROR_ALREADY_EXISTS) {
        const auto utf8 = WideToUtf8(dataDirectory.data(), dataDirectory.size());
        if (utf8.has_value()) {
          engine = slime_create_with_data_dir(
              reinterpret_cast<const std::uint8_t *>(utf8->data()),
              utf8->size());
        }
      }
    }
  } catch (...) {
    engine = nullptr;
  }
  if (engine == nullptr) {
    engine = slime_create();
  }
  ApplyWindowsPreferences(engine, preferences, false);
  return engine;
}

HRESULT LaunchSettingsExecutable() noexcept {
  try {
    std::vector<wchar_t> modulePath(512);
    DWORD length = 0;
    while (true) {
      length = GetModuleFileNameW(g_module, modulePath.data(),
                                  static_cast<DWORD>(modulePath.size()));
      if (length == 0) {
        return HRESULT_FROM_WIN32(GetLastError());
      }
      if (length < modulePath.size()) {
        break;
      }
      if (modulePath.size() >= 32768) {
        return HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
      }
      modulePath.resize(modulePath.size() * 2);
    }
    if (length == 0 || length >= modulePath.size()) {
      return HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);
    }
    std::wstring executable(modulePath.data(), length);
    const std::size_t separator = executable.find_last_of(L"\\/");
    if (separator == std::wstring::npos) {
      return HRESULT_FROM_WIN32(ERROR_PATH_NOT_FOUND);
    }
    executable.resize(separator + 1);
    executable.append(L"SlimeSettings.exe");
    std::wstring commandLine = L"\"" + executable + L"\"";
    STARTUPINFOW startup{};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION process{};
    if (!CreateProcessW(executable.c_str(), commandLine.data(), nullptr, nullptr,
                        FALSE, 0, nullptr, nullptr, &startup, &process)) {
      return HRESULT_FROM_WIN32(GetLastError());
    }
    CloseHandle(process.hThread);
    CloseHandle(process.hProcess);
    return S_OK;
  } catch (...) {
    return E_OUTOFMEMORY;
  }
}

struct EngineKey {
  std::uint32_t kind = 0;
  std::uint32_t value = 0;
  bool valid = false;
};

bool HasCommandModifier() noexcept {
  return (GetKeyState(VK_CONTROL) & 0x8000) != 0 || (GetKeyState(VK_MENU) & 0x8000) != 0 ||
         (GetKeyState(VK_LWIN) & 0x8000) != 0 || (GetKeyState(VK_RWIN) & 0x8000) != 0;
}

bool IsPotentialCharacterKey(const WPARAM virtualKey) noexcept {
  return (virtualKey >= L'0' && virtualKey <= L'9') ||
         (virtualKey >= L'A' && virtualKey <= L'Z') || virtualKey == VK_OEM_1 ||
         virtualKey == VK_OEM_PLUS || virtualKey == VK_OEM_COMMA ||
         virtualKey == VK_OEM_MINUS || virtualKey == VK_OEM_PERIOD ||
         virtualKey == VK_OEM_2 || virtualKey == VK_OEM_3 || virtualKey == VK_OEM_4 ||
         virtualKey == VK_OEM_5 || virtualKey == VK_OEM_6 || virtualKey == VK_OEM_7 ||
         virtualKey == VK_OEM_102;
}

EngineKey TranslateSpecialKey(const WPARAM virtualKey, const bool hasComposition) noexcept {
  if (virtualKey == VK_SPACE && hasComposition) {
    return {SLIME_EVENT_SPACE, 0, true};
  }
  if (!hasComposition) {
    return {};
  }
  switch (virtualKey) {
  case VK_RETURN:
    return {SLIME_EVENT_ENTER, 0, true};
  case VK_ESCAPE:
    return {SLIME_EVENT_ESCAPE, 0, true};
  case VK_BACK:
    return {SLIME_EVENT_BACKSPACE, 0, true};
  case VK_DOWN:
  case VK_NEXT:
    return {SLIME_EVENT_NEXT_CANDIDATE, 0, true};
  case VK_UP:
  case VK_PRIOR:
    return {SLIME_EVENT_PREVIOUS_CANDIDATE, 0, true};
  case VK_F6:
    return {SLIME_EVENT_TRANSFORM_HIRAGANA, 0, true};
  case VK_F7:
    return {SLIME_EVENT_TRANSFORM_FULL_KATAKANA, 0, true};
  case VK_F8:
    return {SLIME_EVENT_TRANSFORM_HALF_KATAKANA, 0, true};
  case VK_F9:
    return {SLIME_EVENT_TRANSFORM_FULL_ALPHANUMERIC, 0, true};
  case VK_F10:
    return {SLIME_EVENT_TRANSFORM_HALF_ALPHANUMERIC, 0, true};
  default:
    return {};
  }
}

EngineKey TranslateKey(const WPARAM virtualKey, const LPARAM keyData,
                       const bool hasComposition) noexcept {
  if (HasCommandModifier()) {
    return {};
  }
  if (const EngineKey special = TranslateSpecialKey(virtualKey, hasComposition); special.valid) {
    return special;
  }
  if (!IsPotentialCharacterKey(virtualKey)) {
    return {};
  }

  BYTE keyboardState[256]{};
  if (!GetKeyboardState(keyboardState)) {
    return {};
  }
  wchar_t characters[4]{};
  const UINT scanCode = static_cast<UINT>((static_cast<ULONG_PTR>(keyData) >> 16U) & 0xffU);
  const int count = ToUnicodeEx(static_cast<UINT>(virtualKey), scanCode, keyboardState, characters,
                                static_cast<int>(std::size(characters)), 0,
                                GetKeyboardLayout(0));
  if (count == 1 && !IS_HIGH_SURROGATE(characters[0]) && !IS_LOW_SURROGATE(characters[0])) {
    return {SLIME_EVENT_CHARACTER, static_cast<std::uint32_t>(characters[0]), true};
  }
  if (count == 2 && IS_SURROGATE_PAIR(characters[0], characters[1])) {
    const auto high = static_cast<std::uint32_t>(characters[0]) - 0xd800U;
    const auto low = static_cast<std::uint32_t>(characters[1]) - 0xdc00U;
    return {SLIME_EVENT_CHARACTER, 0x10000U + (high << 10U) + low, true};
  }
  return {};
}

class EditSession final : public ITfEditSession {
public:
  EditSession(TextService *owner, ITfContext *context, EngineKey key) noexcept;

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **object) override;
  ULONG STDMETHODCALLTYPE AddRef() override { return ++referenceCount_; }
  ULONG STDMETHODCALLTYPE Release() override;
  HRESULT STDMETHODCALLTYPE DoEditSession(TfEditCookie editCookie) override;

  [[nodiscard]] bool handled() const noexcept { return handled_; }

private:
  ~EditSession();

  std::atomic_ulong referenceCount_{1};
  TextService *owner_;
  ComPtr<ITfContext> context_;
  EngineKey key_;
  bool handled_ = false;
};

class ConfigureFunction final : public ITfFnConfigure {
public:
  ConfigureFunction() noexcept { ++g_objectCount; }

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **object) override {
    if (object == nullptr) {
      return E_POINTER;
    }
    *object = nullptr;
    if (iid == IID_IUnknown || iid == IID_ITfFunction ||
        iid == IID_ITfFnConfigure) {
      *object = static_cast<ITfFnConfigure *>(this);
      AddRef();
      return S_OK;
    }
    return E_NOINTERFACE;
  }

  ULONG STDMETHODCALLTYPE AddRef() override { return ++referenceCount_; }

  ULONG STDMETHODCALLTYPE Release() override {
    const ULONG remaining = --referenceCount_;
    if (remaining == 0) {
      delete this;
    }
    return remaining;
  }

  HRESULT STDMETHODCALLTYPE GetDisplayName(BSTR *name) override {
    if (name == nullptr) {
      return E_POINTER;
    }
    *name = SysAllocString(L"Slime settings");
    return *name != nullptr ? S_OK : E_OUTOFMEMORY;
  }

  HRESULT STDMETHODCALLTYPE Show(HWND, LANGID, REFGUID) override {
    return LaunchSettingsExecutable();
  }

private:
  ~ConfigureFunction() { --g_objectCount; }

  std::atomic_ulong referenceCount_{1};
};

class TextService final : public ITfTextInputProcessorEx,
                          public ITfKeyEventSink,
                          public ITfCompositionSink,
                          public ITfFunctionProvider,
                          public ITfFnSearchCandidateProvider {
public:
  TextService() noexcept
      : candidateWindow_(g_module, this,
                         &TextService::CandidateWindowSelection),
        preferencesPath_(WindowsPreferencesPath()) {
    ++g_objectCount;
    engine_ = CreateEngine(preferencesPath_, preferences_);
    preferencesMonitor_.Start(preferencesPath_);
  }

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **object) override;
  ULONG STDMETHODCALLTYPE AddRef() override { return ++referenceCount_; }
  ULONG STDMETHODCALLTYPE Release() override;

  HRESULT STDMETHODCALLTYPE Activate(ITfThreadMgr *threadManager, TfClientId clientId) override;
  HRESULT STDMETHODCALLTYPE ActivateEx(ITfThreadMgr *threadManager,
                                       TfClientId clientId,
                                       DWORD flags) override;
  HRESULT STDMETHODCALLTYPE Deactivate() override;

  HRESULT STDMETHODCALLTYPE OnSetFocus(BOOL foreground) override;
  HRESULT STDMETHODCALLTYPE OnTestKeyDown(ITfContext *context, WPARAM virtualKey, LPARAM keyData,
                                          BOOL *eaten) override;
  HRESULT STDMETHODCALLTYPE OnTestKeyUp(ITfContext *context, WPARAM virtualKey, LPARAM keyData,
                                        BOOL *eaten) override;
  HRESULT STDMETHODCALLTYPE OnKeyDown(ITfContext *context, WPARAM virtualKey, LPARAM keyData,
                                      BOOL *eaten) override;
  HRESULT STDMETHODCALLTYPE OnKeyUp(ITfContext *context, WPARAM virtualKey, LPARAM keyData,
                                    BOOL *eaten) override;
  HRESULT STDMETHODCALLTYPE OnPreservedKey(ITfContext *context, REFGUID key, BOOL *eaten) override;

  HRESULT STDMETHODCALLTYPE OnCompositionTerminated(TfEditCookie editCookie,
                                                    ITfComposition *composition) override;

  HRESULT STDMETHODCALLTYPE GetType(GUID *guid) override;
  HRESULT STDMETHODCALLTYPE GetDescription(BSTR *description) override;
  HRESULT STDMETHODCALLTYPE GetFunction(REFGUID guid, REFIID iid,
                                        IUnknown **function) override;
  HRESULT STDMETHODCALLTYPE GetDisplayName(BSTR *name) override;
  HRESULT STDMETHODCALLTYPE GetSearchCandidates(BSTR query, BSTR applicationId,
                                                ITfCandidateList **list) override;
  HRESULT STDMETHODCALLTYPE SetResult(BSTR query, BSTR applicationId,
                                      BSTR result) override;

  bool ProcessEvent(TfEditCookie editCookie, ITfContext *context, EngineKey key) noexcept;

private:
  ~TextService();
  static void CandidateWindowSelection(void *context, UINT candidateIndex,
                                       bool accept) noexcept;
  static void CandidateUiVisibility(void *context, bool show) noexcept;
  static bool CandidateUiSelection(void *context, UINT candidateIndex,
                                   bool accept) noexcept;
  static bool CandidateUiKey(void *context, WPARAM virtualKey,
                             LPARAM keyData) noexcept;
  static bool CandidateUiAbort(void *context) noexcept;
  [[nodiscard]] EngineKey CandidateNumberKey(WPARAM virtualKey) const noexcept;
  HRESULT RequestEngineEvent(ITfContext *context, EngineKey key,
                             bool &handled) noexcept;
  bool RequestCandidateEvent(UINT candidateIndex, bool accept) noexcept;
  HRESULT EnsureComposition(TfEditCookie editCookie, ITfContext *context) noexcept;
  HRESULT SetCompositionText(TfEditCookie editCookie, const std::wstring &text) noexcept;
  HRESULT CommitText(TfEditCookie editCookie, ITfContext *context,
                     const std::wstring &text) noexcept;
  HRESULT ClearComposition(TfEditCookie editCookie) noexcept;
  bool ResolveCandidatePlacement(TfEditCookie editCookie, ITfContext *context,
                                 RECT &anchor, HWND &owner) noexcept;
  void UpdateCandidateWindow() noexcept;
  void ShowCandidates(TfEditCookie editCookie, ITfContext *context,
                      const EngineAction &action) noexcept;
  void HideCandidates() noexcept;
  void ResetEngineAfterTermination() noexcept;
  void ResetTransientContext() noexcept;
  bool GetSelectionCaret(TfEditCookie editCookie, ITfContext *context,
                         ComPtr<ITfRange> &caret) noexcept;
  bool SelectionBoundaryChanged(TfEditCookie editCookie,
                                ITfContext *context) noexcept;
  void ObserveSelection(TfEditCookie editCookie,
                        ITfContext *context) noexcept;
  void SynchronizeExternalDocumentContext(TfEditCookie editCookie,
                                          ITfContext *context) noexcept;
  void MaybeReloadPreferences(bool force = false) noexcept;

  std::atomic_ulong referenceCount_{1};
  SlimeHandle *engine_ = nullptr;
  ComPtr<ITfThreadMgr> threadManager_;
  ComPtr<ITfComposition> composition_;
  ComPtr<ITfUIElementMgr> uiElementManager_;
  ComPtr<CandidateUiElement> candidateUi_;
  ComPtr<ITfContext> candidateContext_;
  CandidateWindow candidateWindow_;
  RECT candidateAnchor_{};
  HWND candidateOwner_ = nullptr;
  DWORD candidateUiId_ = std::numeric_limits<DWORD>::max();
  bool candidatePlacementValid_ = false;
  TfClientId clientId_ = kInvalidClientId;
  bool hasComposition_ = false;
  bool advised_ = false;
  bool functionProviderAdvised_ = false;
  DWORD activationFlags_ = 0;
  WindowsPreferences preferences_;
  std::wstring preferencesPath_;
  WindowsPreferencesMonitor preferencesMonitor_;
  bool hasSearchQuery_ = false;
  bool needsExternalDocumentContext_ = true;
  ComPtr<ITfContext> observedContext_;
  ComPtr<ITfRange> observedCaret_;
  std::wstring searchQuery_;
  std::wstring searchApplicationId_;
  std::vector<std::wstring> searchCandidates_;
};

EditSession::EditSession(TextService *owner, ITfContext *context, const EngineKey key) noexcept
    : owner_(owner), context_(context), key_(key) {
  ++g_objectCount;
  owner_->AddRef();
}

EditSession::~EditSession() {
  owner_->Release();
  --g_objectCount;
}

HRESULT EditSession::QueryInterface(REFIID iid, void **object) {
  if (object == nullptr) {
    return E_POINTER;
  }
  *object = nullptr;
  if (iid == IID_IUnknown || iid == IID_ITfEditSession) {
    *object = static_cast<ITfEditSession *>(this);
    AddRef();
    return S_OK;
  }
  return E_NOINTERFACE;
}

ULONG EditSession::Release() {
  const ULONG remaining = --referenceCount_;
  if (remaining == 0) {
    delete this;
  }
  return remaining;
}

HRESULT EditSession::DoEditSession(const TfEditCookie editCookie) {
  handled_ = owner_->ProcessEvent(editCookie, context_.Get(), key_);
  return S_OK;
}

HRESULT TextService::QueryInterface(REFIID iid, void **object) {
  if (object == nullptr) {
    return E_POINTER;
  }
  *object = nullptr;
  if (iid == IID_IUnknown || iid == IID_ITfTextInputProcessorEx) {
    *object = static_cast<ITfTextInputProcessorEx *>(this);
  } else if (iid == IID_ITfTextInputProcessor) {
    *object = static_cast<ITfTextInputProcessor *>(this);
  } else if (iid == IID_ITfKeyEventSink) {
    *object = static_cast<ITfKeyEventSink *>(this);
  } else if (iid == IID_ITfCompositionSink) {
    *object = static_cast<ITfCompositionSink *>(this);
  } else if (iid == IID_ITfFunctionProvider) {
    *object = static_cast<ITfFunctionProvider *>(this);
  } else if (iid == IID_ITfFunction || iid == IID_ITfFnSearchCandidateProvider) {
    *object = static_cast<ITfFnSearchCandidateProvider *>(this);
  } else {
    return E_NOINTERFACE;
  }
  AddRef();
  return S_OK;
}

ULONG TextService::Release() {
  const ULONG remaining = --referenceCount_;
  if (remaining == 0) {
    delete this;
  }
  return remaining;
}

TextService::~TextService() {
  HideCandidates();
  if (engine_ != nullptr) {
    slime_destroy(engine_);
  }
  --g_objectCount;
}

HRESULT TextService::Activate(ITfThreadMgr *threadManager, const TfClientId clientId) {
  return ActivateEx(threadManager, clientId, 0);
}

HRESULT TextService::ActivateEx(ITfThreadMgr *threadManager,
                                const TfClientId clientId,
                                const DWORD flags) {
  if (threadManager == nullptr || engine_ == nullptr) {
    return E_INVALIDARG;
  }
  threadManager_ = threadManager;
  clientId_ = clientId;
  activationFlags_ = flags;
  ResetTransientContext();
  MaybeReloadPreferences(true);
  ApplyWindowsPreferences(engine_, preferences_,
                          (activationFlags_ & TF_TMAE_SECUREMODE) != 0);

  ComPtr<ITfKeystrokeMgr> keystrokeManager;
  HRESULT result = threadManager_->QueryInterface(IID_PPV_ARGS(&keystrokeManager));
  if (FAILED(result)) {
    threadManager_.Reset();
    clientId_ = kInvalidClientId;
    activationFlags_ = 0;
    return result;
  }
  result = keystrokeManager->AdviseKeyEventSink(clientId_, this, TRUE);
  if (FAILED(result)) {
    threadManager_.Reset();
    clientId_ = kInvalidClientId;
    activationFlags_ = 0;
    return result;
  }
  advised_ = true;

  ComPtr<ITfSourceSingle> source;
  result = threadManager_->QueryInterface(IID_PPV_ARGS(&source));
  if (SUCCEEDED(result)) {
    auto *unknown = static_cast<IUnknown *>(
        static_cast<ITfTextInputProcessor *>(this));
    result = source->AdviseSingleSink(clientId_, IID_ITfFunctionProvider, unknown);
  }
  if (FAILED(result)) {
    keystrokeManager->UnadviseKeyEventSink(clientId_);
    advised_ = false;
    threadManager_.Reset();
    clientId_ = kInvalidClientId;
    activationFlags_ = 0;
    return result;
  }
  functionProviderAdvised_ = true;
  return S_OK;
}

HRESULT TextService::Deactivate() {
  ResetTransientContext();
  if (functionProviderAdvised_ && threadManager_ != nullptr) {
    ComPtr<ITfSourceSingle> source;
    if (SUCCEEDED(threadManager_->QueryInterface(IID_PPV_ARGS(&source)))) {
      source->UnadviseSingleSink(clientId_, IID_ITfFunctionProvider);
    }
  }
  functionProviderAdvised_ = false;
  if (advised_ && threadManager_ != nullptr) {
    ComPtr<ITfKeystrokeMgr> keystrokeManager;
    if (SUCCEEDED(threadManager_->QueryInterface(IID_PPV_ARGS(&keystrokeManager)))) {
      keystrokeManager->UnadviseKeyEventSink(clientId_);
    }
  }
  advised_ = false;
  clientId_ = kInvalidClientId;
  activationFlags_ = 0;
  HideCandidates();
  composition_.Reset();
  hasComposition_ = false;
  hasSearchQuery_ = false;
  searchQuery_.clear();
  searchApplicationId_.clear();
  searchCandidates_.clear();
  threadManager_.Reset();
  return S_OK;
}

HRESULT TextService::OnSetFocus(const BOOL foreground) {
  if (foreground) {
    MaybeReloadPreferences(true);
  } else {
    ResetTransientContext();
    HideCandidates();
  }
  return S_OK;
}

HRESULT TextService::OnTestKeyDown(ITfContext *, const WPARAM virtualKey, LPARAM, BOOL *eaten) {
  if (eaten == nullptr) {
    return E_POINTER;
  }
  *eaten = CandidateNumberKey(virtualKey).valid ||
                   (!HasCommandModifier() &&
                    (TranslateSpecialKey(virtualKey, hasComposition_).valid ||
                     IsPotentialCharacterKey(virtualKey)))
                ? TRUE
                : FALSE;
  return S_OK;
}

HRESULT TextService::OnTestKeyUp(ITfContext *, WPARAM, LPARAM, BOOL *eaten) {
  if (eaten == nullptr) {
    return E_POINTER;
  }
  *eaten = FALSE;
  return S_OK;
}

HRESULT TextService::OnKeyDown(ITfContext *context, const WPARAM virtualKey, const LPARAM keyData,
                               BOOL *eaten) {
  if (context == nullptr || eaten == nullptr || clientId_ == kInvalidClientId) {
    return E_INVALIDARG;
  }
  *eaten = FALSE;
  if (const EngineKey candidate = CandidateNumberKey(virtualKey); candidate.valid) {
    *eaten = RequestCandidateEvent(candidate.value, true) ? TRUE : FALSE;
    return S_OK;
  }
  const EngineKey key = TranslateKey(virtualKey, keyData, hasComposition_);
  if (!key.valid) {
    return S_OK;
  }

  bool handled = false;
  const HRESULT result = RequestEngineEvent(context, key, handled);
  if (SUCCEEDED(result) && handled) {
    *eaten = TRUE;
  }
  return result;
}

HRESULT TextService::OnKeyUp(ITfContext *, WPARAM, LPARAM, BOOL *eaten) {
  if (eaten == nullptr) {
    return E_POINTER;
  }
  *eaten = FALSE;
  return S_OK;
}

HRESULT TextService::OnPreservedKey(ITfContext *, REFGUID, BOOL *eaten) {
  if (eaten == nullptr) {
    return E_POINTER;
  }
  *eaten = FALSE;
  return S_OK;
}

HRESULT TextService::GetType(GUID *guid) {
  if (guid == nullptr) {
    return E_POINTER;
  }
  *guid = kTextServiceClsid;
  return S_OK;
}

HRESULT TextService::GetDescription(BSTR *description) {
  if (description == nullptr) {
    return E_POINTER;
  }
  *description = SysAllocString(kDescription);
  return *description != nullptr ? S_OK : E_OUTOFMEMORY;
}

HRESULT TextService::GetFunction(REFGUID guid, REFIID iid,
                                 IUnknown **function) {
  if (function == nullptr) {
    return E_POINTER;
  }
  *function = nullptr;
  if (!IsEqualGUID(guid, GUID_NULL)) {
    return E_NOINTERFACE;
  }
  if (iid == IID_ITfFnSearchCandidateProvider) {
    auto *provider = static_cast<ITfFnSearchCandidateProvider *>(this);
    provider->AddRef();
    *function = provider;
    return S_OK;
  }
  if (iid == IID_ITfFnConfigure) {
    auto *provider = new (std::nothrow) ConfigureFunction();
    if (provider == nullptr) {
      return E_OUTOFMEMORY;
    }
    *function = provider;
    return S_OK;
  }
  return E_NOINTERFACE;
}

HRESULT TextService::GetDisplayName(BSTR *name) {
  if (name == nullptr) {
    return E_POINTER;
  }
  *name = SysAllocString(L"Slime search candidates");
  return *name != nullptr ? S_OK : E_OUTOFMEMORY;
}

HRESULT TextService::GetSearchCandidates(BSTR query, BSTR applicationId,
                                         ITfCandidateList **list) {
  if (list == nullptr) {
    return E_POINTER;
  }
  *list = nullptr;
  if (engine_ == nullptr) {
    return E_FAIL;
  }
  MaybeReloadPreferences();
  try {
    hasSearchQuery_ = false;
    searchQuery_.clear();
    searchApplicationId_.clear();
    searchCandidates_.clear();
    const UINT queryLength = query == nullptr ? 0 : SysStringLen(query);
    const UINT applicationIdLength =
        applicationId == nullptr ? 0 : SysStringLen(applicationId);
    if (queryLength == 0) {
      return S_FALSE;
    }
    const auto utf8Query = WideToUtf8(query, queryLength);
    if (!utf8Query.has_value()) {
      return E_INVALIDARG;
    }
    EngineStringCollection collection;
    const std::uint32_t status = slime_conversion_candidates(
        engine_, reinterpret_cast<const std::uint8_t *>(utf8Query->data()),
        utf8Query->size(), &collection, CollectString);
    if (status != SLIME_STATUS_OK) {
      return status == SLIME_STATUS_INVALID_UTF8 ? E_INVALIDARG : E_FAIL;
    }
    if (collection.failed) {
      return E_OUTOFMEMORY;
    }
    auto filtered = FilterSearchCandidates(std::move(collection.values),
                                           kSearchCandidateLimit);
    if (filtered.empty()) {
      return S_FALSE;
    }
    const HRESULT createResult = CreateSearchCandidateList(filtered, list);
    if (FAILED(createResult)) {
      return createResult;
    }
    try {
      searchQuery_.assign(query, queryLength);
      if (applicationId == nullptr) {
        searchApplicationId_.clear();
      } else {
        searchApplicationId_.assign(applicationId, applicationIdLength);
      }
      searchCandidates_ = std::move(filtered);
      hasSearchQuery_ = true;
    } catch (...) {
      (*list)->Release();
      *list = nullptr;
      throw;
    }
    return S_OK;
  } catch (...) {
    return E_OUTOFMEMORY;
  }
}

HRESULT TextService::SetResult(BSTR query, BSTR applicationId, BSTR result) {
  if (!hasSearchQuery_ || query == nullptr || result == nullptr) {
    return E_PENDING;
  }
  try {
    const UINT queryLength = SysStringLen(query);
    const UINT applicationIdLength =
        applicationId == nullptr ? 0 : SysStringLen(applicationId);
    const UINT resultLength = SysStringLen(result);
    const std::wstring_view applicationView =
        applicationId == nullptr
            ? std::wstring_view{}
            : std::wstring_view(applicationId, applicationIdLength);
    if (searchQuery_ != std::wstring_view(query, queryLength) ||
        searchApplicationId_ != applicationView) {
      return E_PENDING;
    }
    const std::wstring_view chosen(result, resultLength);
    if (std::find(searchCandidates_.begin(), searchCandidates_.end(), chosen) ==
        searchCandidates_.end()) {
      return E_INVALIDARG;
    }
    const auto utf8Query = WideToUtf8(query, queryLength);
    const auto utf8Result = WideToUtf8(result, resultLength);
    if (!utf8Query.has_value() || !utf8Result.has_value()) {
      return E_INVALIDARG;
    }
    const std::uint32_t status = slime_record_external_selection(
        engine_, reinterpret_cast<const std::uint8_t *>(utf8Query->data()),
        utf8Query->size(),
        reinterpret_cast<const std::uint8_t *>(utf8Result->data()),
        utf8Result->size());
    if (status == SLIME_STATUS_OK) {
      hasSearchQuery_ = false;
      searchQuery_.clear();
      searchApplicationId_.clear();
      searchCandidates_.clear();
      return S_OK;
    }
    return status == SLIME_STATUS_INVALID_CANDIDATE ? E_INVALIDARG : E_FAIL;
  } catch (...) {
    return E_OUTOFMEMORY;
  }
}

void TextService::CandidateWindowSelection(void *context, const UINT candidateIndex,
                                           const bool accept) noexcept {
  if (context != nullptr) {
    static_cast<TextService *>(context)->RequestCandidateEvent(candidateIndex, accept);
  }
}

EngineKey TextService::CandidateNumberKey(const WPARAM virtualKey) const noexcept {
  if (candidateUi_ == nullptr || HasCommandModifier() ||
      (GetKeyState(VK_SHIFT) & 0x8000) != 0) {
    return {};
  }
  UINT number = 0;
  if (virtualKey >= L'1' && virtualKey <= L'9') {
    number = static_cast<UINT>(virtualKey - L'1');
  } else if (virtualKey >= VK_NUMPAD1 && virtualKey <= VK_NUMPAD9) {
    number = static_cast<UINT>(virtualKey - VK_NUMPAD1);
  } else {
    return {};
  }
  const std::size_t pageStart =
      (candidateUi_->selected() / kSlimeCandidatePageSize) *
      kSlimeCandidatePageSize;
  const std::size_t index = pageStart + number;
  if (index >= candidateUi_->candidates().size() ||
      index > std::numeric_limits<std::uint32_t>::max()) {
    return {};
  }
  return {SLIME_EVENT_SELECT_CANDIDATE, static_cast<std::uint32_t>(index), true};
}

void TextService::CandidateUiVisibility(void *context, const bool show) noexcept {
  if (context == nullptr) {
    return;
  }
  auto *service = static_cast<TextService *>(context);
  if (show) {
    service->UpdateCandidateWindow();
  } else {
    service->candidateWindow_.Hide();
  }
}

bool TextService::CandidateUiSelection(void *context, const UINT candidateIndex,
                                       const bool accept) noexcept {
  return context != nullptr &&
         static_cast<TextService *>(context)->RequestCandidateEvent(
             candidateIndex, accept);
}

bool TextService::CandidateUiKey(void *context, const WPARAM virtualKey,
                                 LPARAM) noexcept {
  if (context == nullptr || HasCommandModifier() ||
      (GetKeyState(VK_SHIFT) & 0x8000) != 0) {
    return false;
  }
  auto *service = static_cast<TextService *>(context);
  if (const EngineKey number = service->CandidateNumberKey(virtualKey);
      number.valid) {
    return service->RequestCandidateEvent(number.value, true);
  }
  if (service->candidateUi_ == nullptr || service->candidateContext_ == nullptr) {
    return false;
  }
  if (virtualKey == VK_RETURN) {
    return service->RequestCandidateEvent(
        static_cast<UINT>(service->candidateUi_->selected()), true);
  }
  if (virtualKey == VK_ESCAPE) {
    return CandidateUiAbort(context);
  }
  const EngineKey key = TranslateSpecialKey(virtualKey, service->hasComposition_);
  if (!key.valid || (key.kind != SLIME_EVENT_NEXT_CANDIDATE &&
                     key.kind != SLIME_EVENT_PREVIOUS_CANDIDATE)) {
    return false;
  }
  bool handled = false;
  return SUCCEEDED(service->RequestEngineEvent(service->candidateContext_.Get(),
                                                key, handled)) &&
         handled;
}

bool TextService::CandidateUiAbort(void *context) noexcept {
  if (context == nullptr) {
    return false;
  }
  auto *service = static_cast<TextService *>(context);
  if (service->candidateContext_ == nullptr) {
    return false;
  }
  bool handled = false;
  const EngineKey escape{SLIME_EVENT_ESCAPE, 0, true};
  return SUCCEEDED(service->RequestEngineEvent(service->candidateContext_.Get(),
                                                escape, handled)) &&
         handled;
}

HRESULT TextService::RequestEngineEvent(ITfContext *context, const EngineKey key,
                                        bool &handled) noexcept {
  handled = false;
  if (context == nullptr || clientId_ == kInvalidClientId || !key.valid) {
    return E_INVALIDARG;
  }
  MaybeReloadPreferences();
  auto *session = new (std::nothrow) EditSession(this, context, key);
  if (session == nullptr) {
    return E_OUTOFMEMORY;
  }
  HRESULT sessionResult = E_FAIL;
  const HRESULT requestResult = context->RequestEditSession(
      clientId_, session, TF_ES_SYNC | TF_ES_READWRITE, &sessionResult);
  handled = SUCCEEDED(requestResult) && SUCCEEDED(sessionResult) && session->handled();
  session->Release();
  if (FAILED(requestResult)) {
    return requestResult;
  }
  return sessionResult;
}

bool TextService::RequestCandidateEvent(const UINT candidateIndex,
                                        const bool accept) noexcept {
  if (candidateContext_ == nullptr) {
    return false;
  }
  bool handled = false;
  const EngineKey selection{SLIME_EVENT_SELECT_CANDIDATE, candidateIndex, true};
  if (FAILED(RequestEngineEvent(candidateContext_.Get(), selection, handled)) || !handled ||
      !accept) {
    return handled;
  }
  const EngineKey acceptance{SLIME_EVENT_ACCEPT_CANDIDATE, 0, true};
  const HRESULT result = RequestEngineEvent(candidateContext_.Get(), acceptance, handled);
  return SUCCEEDED(result) && handled;
}

HRESULT TextService::EnsureComposition(const TfEditCookie editCookie,
                                       ITfContext *context) noexcept {
  if (composition_ != nullptr) {
    return S_OK;
  }
  TF_SELECTION selection{};
  ULONG fetched = 0;
  HRESULT result = context->GetSelection(editCookie, TF_DEFAULT_SELECTION, 1, &selection, &fetched);
  if (FAILED(result) || fetched != 1 || selection.range == nullptr) {
    return FAILED(result) ? result : E_FAIL;
  }

  ComPtr<ITfRange> selectionRange;
  selectionRange.Attach(selection.range);
  ComPtr<ITfContextComposition> contextComposition;
  result = context->QueryInterface(IID_PPV_ARGS(&contextComposition));
  if (FAILED(result)) {
    return result;
  }
  result = contextComposition->StartComposition(editCookie, selectionRange.Get(), this,
                                                &composition_);
  if (SUCCEEDED(result)) {
    hasComposition_ = true;
  }
  return result;
}

HRESULT TextService::SetCompositionText(const TfEditCookie editCookie,
                                        const std::wstring &text) noexcept {
  if (composition_ == nullptr) {
    return E_UNEXPECTED;
  }
  ComPtr<ITfRange> range;
  HRESULT result = composition_->GetRange(&range);
  if (FAILED(result)) {
    return result;
  }
  return range->SetText(editCookie, 0, text.data(), static_cast<LONG>(text.size()));
}

HRESULT TextService::CommitText(const TfEditCookie editCookie, ITfContext *context,
                                const std::wstring &text) noexcept {
  HRESULT result = EnsureComposition(editCookie, context);
  if (FAILED(result)) {
    return result;
  }
  result = SetCompositionText(editCookie, text);
  if (FAILED(result)) {
    return result;
  }
  ComPtr<ITfComposition> composition = composition_;
  composition_.Reset();
  hasComposition_ = false;
  HideCandidates();
  return composition->EndComposition(editCookie);
}

HRESULT TextService::ClearComposition(const TfEditCookie editCookie) noexcept {
  if (composition_ == nullptr) {
    hasComposition_ = false;
    return S_OK;
  }
  HRESULT result = SetCompositionText(editCookie, L"");
  ComPtr<ITfComposition> composition = composition_;
  composition_.Reset();
  hasComposition_ = false;
  HideCandidates();
  const HRESULT endResult = composition->EndComposition(editCookie);
  return FAILED(result) ? result : endResult;
}

bool TextService::ResolveCandidatePlacement(const TfEditCookie editCookie,
                                            ITfContext *context, RECT &anchor,
                                            HWND &owner) noexcept {
  if (context == nullptr) {
    return false;
  }
  ComPtr<ITfContextView> view;
  if (FAILED(context->GetActiveView(&view)) || view == nullptr) {
    return false;
  }
  HWND viewWindow = nullptr;
  view->GetWnd(&viewWindow);
  owner = viewWindow;

  if (composition_ != nullptr) {
    ComPtr<ITfRange> range;
    BOOL clipped = FALSE;
    if (SUCCEEDED(composition_->GetRange(&range)) && range != nullptr &&
        SUCCEEDED(view->GetTextExt(editCookie, range.Get(), &anchor, &clipped)) &&
        anchor.right >= anchor.left && anchor.bottom >= anchor.top) {
      if (anchor.bottom == anchor.top) {
        ++anchor.bottom;
      }
      return true;
    }
  }

  GUITHREADINFO threadInfo{};
  threadInfo.cbSize = sizeof(threadInfo);
  if (GetGUIThreadInfo(0, &threadInfo) && threadInfo.hwndCaret != nullptr) {
    anchor = threadInfo.rcCaret;
    MapWindowPoints(threadInfo.hwndCaret, nullptr,
                    reinterpret_cast<POINT *>(&anchor), 2);
    owner = threadInfo.hwndCaret;
    if (anchor.bottom == anchor.top) {
      anchor.bottom = anchor.top + 1;
    }
    return true;
  }

  RECT screen{};
  if (SUCCEEDED(view->GetScreenExt(&screen))) {
    anchor = RECT{screen.left, screen.top, screen.left + 1, screen.top + 1};
    return true;
  }
  return false;
}

void TextService::UpdateCandidateWindow() noexcept {
  if (candidateUi_ == nullptr || !candidatePlacementValid_) {
    candidateWindow_.Hide();
    return;
  }
  BOOL shown = FALSE;
  if (FAILED(candidateUi_->IsShown(&shown)) || !shown) {
    candidateWindow_.Hide();
    return;
  }
  candidateWindow_.Update(candidateOwner_, candidateAnchor_,
                          candidateUi_->candidates(), candidateUi_->selected());
}

void TextService::ShowCandidates(const TfEditCookie editCookie, ITfContext *context,
                                 const EngineAction &action) noexcept {
  if (context == nullptr || action.candidates.empty() || threadManager_ == nullptr) {
    HideCandidates();
    return;
  }
  try {
    candidateContext_ = context;
    candidatePlacementValid_ = ResolveCandidatePlacement(
        editCookie, context, candidateAnchor_, candidateOwner_);
    if (candidateUi_ != nullptr) {
      candidateUi_->Update(action.candidates, action.selected);
      if (uiElementManager_ != nullptr &&
          candidateUiId_ != std::numeric_limits<DWORD>::max()) {
        uiElementManager_->UpdateUIElement(candidateUiId_);
      }
      UpdateCandidateWindow();
      return;
    }

    ComPtr<ITfDocumentMgr> documentManager;
    if (FAILED(context->GetDocumentMgr(&documentManager)) || documentManager == nullptr) {
      HideCandidates();
      return;
    }
    ComPtr<ITfUIElementMgr> manager;
    if (FAILED(threadManager_->QueryInterface(IID_PPV_ARGS(&manager)))) {
      HideCandidates();
      return;
    }
    auto *element = new (std::nothrow) CandidateUiElement(
        documentManager.Get(), action.candidates, action.selected, this,
        &TextService::CandidateUiVisibility, &TextService::CandidateUiSelection,
        &TextService::CandidateUiKey, &TextService::CandidateUiAbort);
    if (element == nullptr) {
      HideCandidates();
      return;
    }
    ComPtr<CandidateUiElement> candidate;
    candidate.Attach(element);
    BOOL showServiceUi = TRUE;
    DWORD identifier = std::numeric_limits<DWORD>::max();
    if (FAILED(manager->BeginUIElement(candidate.Get(), &showServiceUi, &identifier))) {
      HideCandidates();
      return;
    }
    candidateUiId_ = identifier;
    uiElementManager_ = std::move(manager);
    candidateUi_ = std::move(candidate);
    candidateUi_->Show(showServiceUi);
  } catch (...) {
    HideCandidates();
  }
}

void TextService::HideCandidates() noexcept {
  if (candidateUi_ != nullptr) {
    candidateUi_->Show(FALSE);
  }
  candidateWindow_.Hide();
  if (uiElementManager_ != nullptr &&
      candidateUiId_ != std::numeric_limits<DWORD>::max()) {
    uiElementManager_->EndUIElement(candidateUiId_);
  }
  candidateUiId_ = std::numeric_limits<DWORD>::max();
  candidateUi_.Reset();
  uiElementManager_.Reset();
  candidateContext_.Reset();
  candidateOwner_ = nullptr;
  candidatePlacementValid_ = false;
}

bool TextService::ProcessEvent(const TfEditCookie editCookie, ITfContext *context,
                               const EngineKey key) noexcept {
  if (engine_ == nullptr || context == nullptr) {
    return false;
  }
  if (!hasComposition_ && key.kind == SLIME_EVENT_CHARACTER) {
    if (SelectionBoundaryChanged(editCookie, context)) {
      slime_reset_context(engine_);
      needsExternalDocumentContext_ = true;
    }
    if (needsExternalDocumentContext_) {
      SynchronizeExternalDocumentContext(editCookie, context);
      needsExternalDocumentContext_ = false;
    }
  }

  EngineActionCollection collection;
  const std::uint32_t status = slime_process_actions_v2(
      engine_, key.kind, key.value, &collection, CollectActionV2);
  if (status != SLIME_STATUS_OK) {
    return false;
  }
  if (collection.failed) {
    ClearComposition(editCookie);
    ResetEngineAfterTermination();
    ObserveSelection(editCookie, context);
    return true;
  }
  const auto &actions = collection.actions;
  for (const auto &action : actions) {
    if (action.kind == SLIME_ACTION_FORWARD_KEY) {
      ObserveSelection(editCookie, context);
      return false;
    }
  }

  for (const auto &action : actions) {
    HRESULT result = S_OK;
    switch (action.kind) {
    case SLIME_ACTION_UPDATE_PREEDIT:
      result = EnsureComposition(editCookie, context);
      if (SUCCEEDED(result)) {
        result = SetCompositionText(editCookie, action.text);
      }
      break;
    case SLIME_ACTION_COMMIT:
      result = CommitText(editCookie, context, action.text);
      break;
    case SLIME_ACTION_CLEAR:
      result = ClearComposition(editCookie);
      break;
    case SLIME_ACTION_SHOW_CANDIDATES:
      ShowCandidates(editCookie, context, action);
      break;
    case SLIME_ACTION_HIDE_CANDIDATES:
      HideCandidates();
      break;
    default:
      break;
    }
    if (FAILED(result)) {
      // The Rust engine has already accepted this key. Do not let the host
      // insert the raw key as well; terminate both sides of the composition so
      // the next key starts from a consistent state.
      ClearComposition(editCookie);
      ResetEngineAfterTermination();
      ObserveSelection(editCookie, context);
      return true;
    }
  }
  if (!hasComposition_) {
    ObserveSelection(editCookie, context);
  }
  return true;
}

void TextService::ResetEngineAfterTermination() noexcept {
  if (engine_ == nullptr) {
    return;
  }
  slime_process_actions_v2(engine_, SLIME_EVENT_ESCAPE, 0, nullptr,
                           IgnoreActionV2);
  slime_process_actions_v2(engine_, SLIME_EVENT_ESCAPE, 0, nullptr,
                           IgnoreActionV2);
  ResetTransientContext();
}

void TextService::ResetTransientContext() noexcept {
  if (engine_ != nullptr) {
    slime_reset_context(engine_);
  }
  needsExternalDocumentContext_ = true;
  observedCaret_.Reset();
  observedContext_.Reset();
}

bool TextService::GetSelectionCaret(const TfEditCookie editCookie,
                                    ITfContext *context,
                                    ComPtr<ITfRange> &caret) noexcept {
  caret.Reset();
  if (context == nullptr) {
    return false;
  }
  TF_SELECTION selection{};
  ULONG fetched = 0;
  const HRESULT selectionResult = context->GetSelection(
      editCookie, TF_DEFAULT_SELECTION, 1, &selection, &fetched);
  if (FAILED(selectionResult) || fetched != 1 || selection.range == nullptr) {
    return false;
  }
  caret.Attach(selection.range);
  if (FAILED(caret->Collapse(editCookie, TF_ANCHOR_START))) {
    caret.Reset();
    return false;
  }
  return true;
}

bool TextService::SelectionBoundaryChanged(const TfEditCookie editCookie,
                                           ITfContext *context) noexcept {
  ComPtr<ITfRange> currentCaret;
  if (!GetSelectionCaret(editCookie, context, currentCaret) ||
      observedContext_.Get() != context || observedCaret_ == nullptr) {
    return true;
  }
  LONG comparison = 0;
  return FAILED(currentCaret->CompareStart(
             editCookie, observedCaret_.Get(), TF_ANCHOR_START, &comparison)) ||
         comparison != 0;
}

void TextService::ObserveSelection(const TfEditCookie editCookie,
                                   ITfContext *context) noexcept {
  ComPtr<ITfRange> currentCaret;
  if (!GetSelectionCaret(editCookie, context, currentCaret)) {
    observedCaret_.Reset();
    observedContext_.Reset();
    needsExternalDocumentContext_ = true;
    return;
  }
  currentCaret->SetGravity(editCookie, TF_GRAVITY_BACKWARD,
                           TF_GRAVITY_BACKWARD);
  observedContext_ = context;
  observedCaret_ = std::move(currentCaret);
}

void TextService::SynchronizeExternalDocumentContext(
    const TfEditCookie editCookie, ITfContext *context) noexcept {
  if (engine_ == nullptr) {
    return;
  }
  if ((activationFlags_ & TF_TMAE_SECUREMODE) != 0) {
    slime_reset_context(engine_);
    return;
  }

  ComPtr<ITfRange> contextRange;
  if (!GetSelectionCaret(editCookie, context, contextRange)) {
    slime_set_external_context(engine_, nullptr, 0, nullptr, 0);
    return;
  }
  ComPtr<ITfRange> rightRange;
  if (FAILED(contextRange->Clone(rightRange.GetAddressOf()))) {
    rightRange.Reset();
  }
  LONG shifted = 0;
  if (FAILED(contextRange->ShiftStart(editCookie, -kDocumentContextUtf16Limit,
                                      &shifted, nullptr))) {
    slime_set_external_context(engine_, nullptr, 0, nullptr, 0);
    return;
  }
  std::array<wchar_t,
             static_cast<std::size_t>(kDocumentContextUtf16Limit)>
      text{};
  ULONG length = 0;
  if (FAILED(contextRange->GetText(editCookie, 0, text.data(),
                                   static_cast<ULONG>(text.size()), &length))) {
    slime_set_external_context(engine_, nullptr, 0, nullptr, 0);
    return;
  }
  const auto leftUtf8 = WideToUtf8(text.data(), length);
  if (!leftUtf8.has_value()) {
    slime_set_external_context(engine_, nullptr, 0, nullptr, 0);
    return;
  }
  std::string rightUtf8;
  if (rightRange != nullptr) {
    LONG rightShifted = 0;
    if (SUCCEEDED(rightRange->ShiftEnd(editCookie, kDocumentContextUtf16Limit,
                                       &rightShifted, nullptr))) {
      std::array<wchar_t,
                 static_cast<std::size_t>(kDocumentContextUtf16Limit)>
          rightText{};
      ULONG rightLength = 0;
      if (SUCCEEDED(rightRange->GetText(
              editCookie, 0, rightText.data(),
              static_cast<ULONG>(rightText.size()), &rightLength))) {
        const auto converted = WideToUtf8(rightText.data(), rightLength);
        if (converted.has_value()) {
          rightUtf8 = *converted;
        }
      }
    }
  }
  slime_set_external_context(
      engine_, reinterpret_cast<const std::uint8_t *>(leftUtf8->data()),
      leftUtf8->size(),
      reinterpret_cast<const std::uint8_t *>(rightUtf8.data()),
      rightUtf8.size());
}

void TextService::MaybeReloadPreferences(const bool force) noexcept {
  if (engine_ == nullptr || hasComposition_ || preferencesPath_.empty()) {
    return;
  }
  if (!force && !preferencesMonitor_.HasChanged()) {
    return;
  }

  WindowsPreferences loaded;
  const WindowsPreferencesLoadStatus status =
      LoadWindowsPreferences(preferencesPath_, loaded);
  if (status == WindowsPreferencesLoadStatus::notFound) {
    loaded = WindowsPreferences{};
  } else if (status != WindowsPreferencesLoadStatus::loaded) {
    return;
  }
  if (loaded == preferences_) {
    return;
  }
  preferences_ = loaded;
  ApplyWindowsPreferences(engine_, preferences_,
                          (activationFlags_ & TF_TMAE_SECUREMODE) != 0);
}

HRESULT TextService::OnCompositionTerminated(TfEditCookie, ITfComposition *) {
  HideCandidates();
  composition_.Reset();
  hasComposition_ = false;
  ResetEngineAfterTermination();
  return S_OK;
}

class ClassFactory final : public IClassFactory {
public:
  ClassFactory() noexcept { ++g_objectCount; }

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **object) override {
    if (object == nullptr) {
      return E_POINTER;
    }
    *object = nullptr;
    if (iid == IID_IUnknown || iid == IID_IClassFactory) {
      *object = static_cast<IClassFactory *>(this);
      AddRef();
      return S_OK;
    }
    return E_NOINTERFACE;
  }

  ULONG STDMETHODCALLTYPE AddRef() override { return ++referenceCount_; }

  ULONG STDMETHODCALLTYPE Release() override {
    const ULONG remaining = --referenceCount_;
    if (remaining == 0) {
      delete this;
    }
    return remaining;
  }

  HRESULT STDMETHODCALLTYPE CreateInstance(IUnknown *outer, REFIID iid, void **object) override {
    if (object == nullptr) {
      return E_POINTER;
    }
    *object = nullptr;
    if (outer != nullptr) {
      return CLASS_E_NOAGGREGATION;
    }
    auto *service = new (std::nothrow) TextService();
    if (service == nullptr) {
      return E_OUTOFMEMORY;
    }
    const HRESULT result = service->QueryInterface(iid, object);
    service->Release();
    return result;
  }

  HRESULT STDMETHODCALLTYPE LockServer(const BOOL lock) override {
    if (lock) {
      ++g_serverLocks;
    } else {
      --g_serverLocks;
    }
    return S_OK;
  }

private:
  ~ClassFactory() { --g_objectCount; }
  std::atomic_ulong referenceCount_{1};
};

std::wstring GuidString(const GUID &guid) {
  wchar_t value[39]{};
  return StringFromGUID2(guid, value, static_cast<int>(std::size(value))) > 0 ? value : L"";
}

HRESULT ModulePath(std::wstring &path) {
  std::vector<wchar_t> buffer(MAX_PATH);
  for (;;) {
    const DWORD length = GetModuleFileNameW(g_module, buffer.data(), static_cast<DWORD>(buffer.size()));
    if (length == 0) {
      return HRESULT_FROM_WIN32(GetLastError());
    }
    if (length < buffer.size() - 1) {
      path.assign(buffer.data(), length);
      return S_OK;
    }
    buffer.resize(buffer.size() * 2);
  }
}

HRESULT SetRegistryString(HKEY root, const std::wstring &subkey, const wchar_t *name,
                          const std::wstring &value) {
  HKEY key = nullptr;
  const LONG createResult = RegCreateKeyExW(root, subkey.c_str(), 0, nullptr, 0, KEY_WRITE, nullptr,
                                           &key, nullptr);
  if (createResult != ERROR_SUCCESS) {
    return HRESULT_FROM_WIN32(createResult);
  }
  const DWORD bytes = static_cast<DWORD>((value.size() + 1) * sizeof(wchar_t));
  const LONG setResult = RegSetValueExW(key, name, 0, REG_SZ,
                                       reinterpret_cast<const BYTE *>(value.c_str()), bytes);
  RegCloseKey(key);
  return HRESULT_FROM_WIN32(setResult);
}

HRESULT RegisterComServer() {
  std::wstring modulePath;
  HRESULT result = ModulePath(modulePath);
  if (FAILED(result)) {
    return result;
  }
  const std::wstring classKey = L"CLSID\\" + GuidString(kTextServiceClsid);
  result = SetRegistryString(HKEY_CLASSES_ROOT, classKey, nullptr, kDescription);
  if (FAILED(result)) {
    return result;
  }
  result = SetRegistryString(HKEY_CLASSES_ROOT, classKey + L"\\InprocServer32", nullptr,
                             modulePath);
  if (FAILED(result)) {
    return result;
  }
  return SetRegistryString(HKEY_CLASSES_ROOT, classKey + L"\\InprocServer32", L"ThreadingModel",
                           L"Apartment");
}

void UnregisterComServer() noexcept {
  const std::wstring classKey = L"CLSID\\" + GuidString(kTextServiceClsid);
  RegDeleteTreeW(HKEY_CLASSES_ROOT, classKey.c_str());
}

HRESULT RegisterTsfProfile() {
  ComPtr<ITfInputProcessorProfiles> profiles;
  HRESULT result = CoCreateInstance(CLSID_TF_InputProcessorProfiles, nullptr, CLSCTX_INPROC_SERVER,
                                    IID_PPV_ARGS(&profiles));
  if (FAILED(result)) {
    return result;
  }
  result = profiles->Register(kTextServiceClsid);
  if (FAILED(result)) {
    return result;
  }
  std::wstring modulePath;
  result = ModulePath(modulePath);
  if (FAILED(result)) {
    return result;
  }
  result = profiles->AddLanguageProfile(
      kTextServiceClsid, kJapaneseLanguage, kLanguageProfileGuid, kDescription,
      static_cast<ULONG>(std::size(kDescription) - 1), modulePath.c_str(),
      static_cast<ULONG>(modulePath.size()), 0);
  if (FAILED(result)) {
    return result;
  }

  ComPtr<ITfCategoryMgr> categories;
  result = CoCreateInstance(CLSID_TF_CategoryMgr, nullptr, CLSCTX_INPROC_SERVER,
                            IID_PPV_ARGS(&categories));
  if (FAILED(result)) {
    return result;
  }
  const GUID categoryIds[] = {GUID_TFCAT_TIP_KEYBOARD, kUiElementEnabledCategory,
                              kImmersiveSupportCategory, kSystraySupportCategory};
  for (const GUID &category : categoryIds) {
    result = categories->RegisterCategory(kTextServiceClsid, category, kTextServiceClsid);
    if (FAILED(result)) {
      return result;
    }
  }
  return S_OK;
}

void UnregisterTsfProfile() noexcept {
  ComPtr<ITfCategoryMgr> categories;
  if (SUCCEEDED(CoCreateInstance(CLSID_TF_CategoryMgr, nullptr, CLSCTX_INPROC_SERVER,
                                 IID_PPV_ARGS(&categories)))) {
    const GUID categoryIds[] = {GUID_TFCAT_TIP_KEYBOARD, kUiElementEnabledCategory,
                                kImmersiveSupportCategory, kSystraySupportCategory};
    for (const GUID &category : categoryIds) {
      categories->UnregisterCategory(kTextServiceClsid, category, kTextServiceClsid);
    }
  }
  ComPtr<ITfInputProcessorProfiles> profiles;
  if (SUCCEEDED(CoCreateInstance(CLSID_TF_InputProcessorProfiles, nullptr, CLSCTX_INPROC_SERVER,
                                 IID_PPV_ARGS(&profiles)))) {
    profiles->RemoveLanguageProfile(kTextServiceClsid, kJapaneseLanguage, kLanguageProfileGuid);
    profiles->Unregister(kTextServiceClsid);
  }
}

} // namespace

BOOL APIENTRY DllMain(HMODULE module, const DWORD reason, LPVOID) {
  if (reason == DLL_PROCESS_ATTACH) {
    g_module = module;
    DisableThreadLibraryCalls(module);
  }
  return TRUE;
}

extern "C" HRESULT __stdcall DllCanUnloadNow() {
  return g_objectCount.load() == 0 && SearchCandidateObjectCount() == 0 &&
                 CandidateAutomationObjectCount() == 0 &&
                 g_serverLocks.load() == 0
             ? S_OK
             : S_FALSE;
}

extern "C" HRESULT __stdcall DllGetClassObject(REFCLSID classId, REFIID interfaceId,
                                                void **object) {
  if (object == nullptr) {
    return E_POINTER;
  }
  *object = nullptr;
  if (classId != kTextServiceClsid) {
    return CLASS_E_CLASSNOTAVAILABLE;
  }
  auto *factory = new (std::nothrow) ClassFactory();
  if (factory == nullptr) {
    return E_OUTOFMEMORY;
  }
  const HRESULT result = factory->QueryInterface(interfaceId, object);
  factory->Release();
  return result;
}

extern "C" HRESULT __stdcall DllRegisterServer() {
  const HRESULT initializeResult = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  if (FAILED(initializeResult) && initializeResult != RPC_E_CHANGED_MODE) {
    return initializeResult;
  }
  const bool uninitialize = SUCCEEDED(initializeResult);
  HRESULT result = RegisterComServer();
  if (SUCCEEDED(result)) {
    result = RegisterTsfProfile();
  }
  if (FAILED(result)) {
    UnregisterTsfProfile();
    UnregisterComServer();
  }
  if (uninitialize) {
    CoUninitialize();
  }
  return result;
}

extern "C" HRESULT __stdcall DllUnregisterServer() {
  const HRESULT initializeResult = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
  const bool uninitialize = SUCCEEDED(initializeResult);
  UnregisterTsfProfile();
  const std::wstring classKey = L"CLSID\\" + GuidString(kTextServiceClsid);
  const LONG registryResult = RegDeleteTreeW(HKEY_CLASSES_ROOT, classKey.c_str());
  if (uninitialize) {
    CoUninitialize();
  }
  return registryResult == ERROR_SUCCESS || registryResult == ERROR_FILE_NOT_FOUND
             ? S_OK
             : HRESULT_FROM_WIN32(registryResult);
}
