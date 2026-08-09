#include "CandidateWindow.h"
#include "slime_ffi.h"

// Declare COM base interfaces before the generated accessibility providers.
#include <unknwn.h>
#include <oaidl.h>
#include <UIAutomation.h>
#include <windowsx.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <limits>
#include <mutex>
#include <new>
#include <utility>

namespace {

constexpr wchar_t kWindowClassName[] = L"SlimeIME.CandidateWindow";
constexpr int kHorizontalPadding = 10;
constexpr int kVerticalPadding = 4;
constexpr int kAnnotationGap = 16;
constexpr int kMinimumContentWidth = 180;
constexpr UINT kAutomationSelectMessage = WM_APP + 0x534c;
constexpr HRESULT kElementNotAvailable =
    static_cast<HRESULT>(UIA_E_ELEMENTNOTAVAILABLE);
constexpr HRESULT kOperationNotSupported =
    static_cast<HRESULT>(UIA_E_NOTSUPPORTED);
constexpr HRESULT kInvalidOperation =
    static_cast<HRESULT>(UIA_E_INVALIDOPERATION);

std::atomic_long g_candidateAutomationObjectCount{0};

std::wstring CandidateAnnotationText(const std::uint32_t annotation,
                                     const std::wstring &detail) {
  switch (annotation) {
  case SLIME_CANDIDATE_ANNOTATION_USER_DICTIONARY:
    return L"ユーザー辞書";
  case SLIME_CANDIDATE_ANNOTATION_HISTORY:
    return L"履歴";
  case SLIME_CANDIDATE_ANNOTATION_CORRECTION:
    return detail.empty() ? L"訂正" : detail + L"に訂正";
  case SLIME_CANDIDATE_ANNOTATION_COMPLETION:
    return L"補完";
  case SLIME_CANDIDATE_ANNOTATION_DATE_TIME:
    return L"日付・時刻";
  case SLIME_CANDIDATE_ANNOTATION_NUMBER:
    return L"数値";
  case SLIME_CANDIDATE_ANNOTATION_CONTEXT:
    return L"文脈";
  default:
    return {};
  }
}

std::array<wchar_t, 4> CandidatePrefix(const std::size_t candidateIndex,
                                       const std::size_t pageStart) noexcept {
  const auto number = static_cast<wchar_t>(L'1' + candidateIndex - pageStart);
  return {number, L'.', L' ', L'\0'};
}

bool RegisterCandidateWindowClass(HINSTANCE instance) noexcept {
  WNDCLASSEXW windowClass{};
  windowClass.cbSize = sizeof(windowClass);
  windowClass.style = CS_DBLCLKS;
  windowClass.lpfnWndProc = CandidateWindow::WindowProcedure;
  windowClass.hInstance = instance;
  windowClass.hCursor = LoadCursorW(nullptr, IDC_ARROW);
  windowClass.hbrBackground = GetSysColorBrush(COLOR_WINDOW);
  windowClass.lpszClassName = kWindowClassName;
  if (RegisterClassExW(&windowClass) != 0) {
    return true;
  }
  return GetLastError() == ERROR_CLASS_ALREADY_EXISTS;
}

int ClampCoordinate(const int value, const int lower, const int upper) noexcept {
  if (upper < lower) {
    return lower;
  }
  return std::clamp(value, lower, upper);
}

HRESULT SetVariantString(VARIANT *value, const wchar_t *text) noexcept {
  value->vt = VT_BSTR;
  value->bstrVal = SysAllocString(text);
  return value->bstrVal != nullptr ? S_OK : E_OUTOFMEMORY;
}

HRESULT SetVariantInt(VARIANT *value, const int number) noexcept {
  value->vt = VT_I4;
  value->lVal = number;
  return S_OK;
}

HRESULT SetVariantBool(VARIANT *value, const bool enabled) noexcept {
  value->vt = VT_BOOL;
  value->boolVal = enabled ? VARIANT_TRUE : VARIANT_FALSE;
  return S_OK;
}

} // namespace

CandidatePresentation BuildCandidatePresentation(
    std::wstring value, const std::uint32_t annotation, std::wstring detail) {
  CandidatePresentation presentation;
  presentation.value = std::move(value);
  presentation.annotation = CandidateAnnotationText(annotation, detail);
  presentation.accessibleName = presentation.value;
  if (!presentation.annotation.empty()) {
    presentation.accessibleName += L"、";
    presentation.accessibleName += presentation.annotation;
  }
  return presentation;
}

class CandidateAutomationItem;

class CandidateAutomationRoot final : public IRawElementProviderSimple,
                                      public IRawElementProviderFragment,
                                      public IRawElementProviderFragmentRoot,
                                      public ISelectionProvider {
public:
  CandidateAutomationRoot() noexcept { ++g_candidateAutomationObjectCount; }

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **object) override;
  ULONG STDMETHODCALLTYPE AddRef() override { return ++referenceCount_; }
  ULONG STDMETHODCALLTYPE Release() override;

  HRESULT STDMETHODCALLTYPE get_ProviderOptions(
      ProviderOptions *options) override;
  HRESULT STDMETHODCALLTYPE GetPatternProvider(PATTERNID patternId,
                                               IUnknown **provider) override;
  HRESULT STDMETHODCALLTYPE GetPropertyValue(PROPERTYID propertyId,
                                             VARIANT *value) override;
  HRESULT STDMETHODCALLTYPE get_HostRawElementProvider(
      IRawElementProviderSimple **provider) override;

  HRESULT STDMETHODCALLTYPE Navigate(
      NavigateDirection direction,
      IRawElementProviderFragment **provider) override;
  HRESULT STDMETHODCALLTYPE GetRuntimeId(SAFEARRAY **runtimeId) override;
  HRESULT STDMETHODCALLTYPE get_BoundingRectangle(UiaRect *rectangle) override;
  HRESULT STDMETHODCALLTYPE GetEmbeddedFragmentRoots(SAFEARRAY **roots) override;
  HRESULT STDMETHODCALLTYPE SetFocus() override;
  HRESULT STDMETHODCALLTYPE get_FragmentRoot(
      IRawElementProviderFragmentRoot **root) override;

  HRESULT STDMETHODCALLTYPE ElementProviderFromPoint(
      double x, double y, IRawElementProviderFragment **provider) override;
  HRESULT STDMETHODCALLTYPE GetFocus(
      IRawElementProviderFragment **provider) override;

  HRESULT STDMETHODCALLTYPE GetSelection(SAFEARRAY **selection) override;
  HRESULT STDMETHODCALLTYPE get_CanSelectMultiple(BOOL *multiple) override;
  HRESULT STDMETHODCALLTYPE get_IsSelectionRequired(BOOL *required) override;

  void Update(HWND window,
              const std::vector<CandidatePresentation> &candidates,
              std::size_t selected, std::size_t pageStart, UINT rowHeight,
              int contentWidth) noexcept;
  void SetVisible(bool visible) noexcept;
  void Disconnect() noexcept;
  void RaiseOpened() noexcept;
  void RaiseClosed() noexcept;
  void RaiseSelectionChanged() noexcept;
  HRESULT Select(std::size_t index) noexcept;
  HRESULT CandidateName(std::size_t index, std::wstring &name) noexcept;
  HRESULT IsSelected(std::size_t index, BOOL *selected) noexcept;
  HRESULT CandidateRectangle(std::size_t index, UiaRect *rectangle) noexcept;
  HRESULT VisibleRange(std::size_t &start, std::size_t &end) noexcept;
  HRESULT CreateItem(std::size_t index,
                     IRawElementProviderFragment **provider) noexcept;
  HRESULT CreateSimpleItem(std::size_t index,
                           IRawElementProviderSimple **provider) noexcept;

private:
  ~CandidateAutomationRoot() { --g_candidateAutomationObjectCount; }

  std::atomic_ulong referenceCount_{1};
  std::mutex mutex_;
  HWND window_ = nullptr;
  std::vector<std::wstring> candidates_;
  std::size_t selected_ = 0;
  std::size_t pageStart_ = 0;
  UINT rowHeight_ = 0;
  int contentWidth_ = 0;
  bool visible_ = false;
};

class CandidateAutomationItem final : public IRawElementProviderSimple,
                                      public IRawElementProviderFragment,
                                      public ISelectionItemProvider {
public:
  CandidateAutomationItem(CandidateAutomationRoot *root,
                          const std::size_t index) noexcept
      : root_(root), index_(index) {
    ++g_candidateAutomationObjectCount;
    root_->AddRef();
  }

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **object) override {
    if (object == nullptr) {
      return E_POINTER;
    }
    *object = nullptr;
    if (iid == IID_IUnknown || iid == IID_IRawElementProviderSimple) {
      *object = static_cast<IRawElementProviderSimple *>(this);
    } else if (iid == IID_IRawElementProviderFragment) {
      *object = static_cast<IRawElementProviderFragment *>(this);
    } else if (iid == IID_ISelectionItemProvider) {
      *object = static_cast<ISelectionItemProvider *>(this);
    } else {
      return E_NOINTERFACE;
    }
    AddRef();
    return S_OK;
  }

  ULONG STDMETHODCALLTYPE AddRef() override { return ++referenceCount_; }

  ULONG STDMETHODCALLTYPE Release() override {
    const ULONG remaining = --referenceCount_;
    if (remaining == 0) {
      delete this;
    }
    return remaining;
  }

  HRESULT STDMETHODCALLTYPE get_ProviderOptions(
      ProviderOptions *options) override {
    if (options == nullptr) {
      return E_POINTER;
    }
    *options = ProviderOptions_ServerSideProvider;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE GetPatternProvider(PATTERNID patternId,
                                               IUnknown **provider) override {
    if (provider == nullptr) {
      return E_POINTER;
    }
    *provider = nullptr;
    if (patternId == UIA_SelectionItemPatternId) {
      *provider = static_cast<ISelectionItemProvider *>(this);
      AddRef();
    }
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE GetPropertyValue(PROPERTYID propertyId,
                                             VARIANT *value) override {
    if (value == nullptr) {
      return E_POINTER;
    }
    VariantInit(value);
    if (propertyId == UIA_NamePropertyId) {
      std::wstring name;
      const HRESULT result = root_->CandidateName(index_, name);
      return SUCCEEDED(result) ? SetVariantString(value, name.c_str()) : result;
    }
    if (propertyId == UIA_ControlTypePropertyId) {
      return SetVariantInt(value, UIA_ListItemControlTypeId);
    }
    if (propertyId == UIA_SelectionItemIsSelectedPropertyId) {
      BOOL selected = FALSE;
      const HRESULT result = root_->IsSelected(index_, &selected);
      return SUCCEEDED(result) ? SetVariantBool(value, selected != FALSE)
                               : result;
    }
    if (propertyId == UIA_IsEnabledPropertyId ||
        propertyId == UIA_IsControlElementPropertyId ||
        propertyId == UIA_IsContentElementPropertyId) {
      return SetVariantBool(value, true);
    }
    if (propertyId == UIA_IsOffscreenPropertyId) {
      UiaRect rectangle{};
      return SetVariantBool(value,
                            FAILED(root_->CandidateRectangle(index_, &rectangle)));
    }
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE get_HostRawElementProvider(
      IRawElementProviderSimple **provider) override {
    if (provider == nullptr) {
      return E_POINTER;
    }
    *provider = nullptr;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE Navigate(
      NavigateDirection direction,
      IRawElementProviderFragment **provider) override {
    if (provider == nullptr) {
      return E_POINTER;
    }
    *provider = nullptr;
    if (direction == NavigateDirection_Parent) {
      *provider = static_cast<IRawElementProviderFragment *>(root_);
      root_->AddRef();
      return S_OK;
    }
    std::size_t start = 0;
    std::size_t end = 0;
    const HRESULT result = root_->VisibleRange(start, end);
    if (FAILED(result)) {
      return result;
    }
    if (direction == NavigateDirection_NextSibling && index_ + 1 < end) {
      return root_->CreateItem(index_ + 1, provider);
    }
    if (direction == NavigateDirection_PreviousSibling && index_ > start) {
      return root_->CreateItem(index_ - 1, provider);
    }
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE GetRuntimeId(SAFEARRAY **runtimeId) override {
    if (runtimeId == nullptr) {
      return E_POINTER;
    }
    *runtimeId = SafeArrayCreateVector(VT_I4, 0, 2);
    if (*runtimeId == nullptr) {
      return E_OUTOFMEMORY;
    }
    LONG *values = nullptr;
    const HRESULT result = SafeArrayAccessData(
        *runtimeId, reinterpret_cast<void **>(&values));
    if (FAILED(result)) {
      SafeArrayDestroy(*runtimeId);
      *runtimeId = nullptr;
      return result;
    }
    values[0] = UiaAppendRuntimeId;
    values[1] = static_cast<LONG>(index_ + 1);
    SafeArrayUnaccessData(*runtimeId);
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE get_BoundingRectangle(UiaRect *rectangle) override {
    return root_->CandidateRectangle(index_, rectangle);
  }

  HRESULT STDMETHODCALLTYPE GetEmbeddedFragmentRoots(SAFEARRAY **roots) override {
    if (roots == nullptr) {
      return E_POINTER;
    }
    *roots = nullptr;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE SetFocus() override { return Select(); }

  HRESULT STDMETHODCALLTYPE get_FragmentRoot(
      IRawElementProviderFragmentRoot **root) override {
    if (root == nullptr) {
      return E_POINTER;
    }
    *root = static_cast<IRawElementProviderFragmentRoot *>(root_);
    root_->AddRef();
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE Select() override { return root_->Select(index_); }
  HRESULT STDMETHODCALLTYPE AddToSelection() override {
    return root_->Select(index_);
  }
  HRESULT STDMETHODCALLTYPE RemoveFromSelection() override {
    BOOL selected = FALSE;
    const HRESULT result = root_->IsSelected(index_, &selected);
    return FAILED(result) ? result
                          : (selected != FALSE ? kInvalidOperation : S_OK);
  }
  HRESULT STDMETHODCALLTYPE get_IsSelected(BOOL *selected) override {
    return root_->IsSelected(index_, selected);
  }
  HRESULT STDMETHODCALLTYPE get_SelectionContainer(
      IRawElementProviderSimple **container) override {
    if (container == nullptr) {
      return E_POINTER;
    }
    *container = static_cast<IRawElementProviderSimple *>(root_);
    root_->AddRef();
    return S_OK;
  }

private:
  ~CandidateAutomationItem() {
    root_->Release();
    --g_candidateAutomationObjectCount;
  }

  std::atomic_ulong referenceCount_{1};
  CandidateAutomationRoot *root_;
  std::size_t index_;
};

HRESULT CandidateAutomationRoot::QueryInterface(REFIID iid, void **object) {
  if (object == nullptr) {
    return E_POINTER;
  }
  *object = nullptr;
  if (iid == IID_IUnknown || iid == IID_IRawElementProviderSimple) {
    *object = static_cast<IRawElementProviderSimple *>(this);
  } else if (iid == IID_IRawElementProviderFragment) {
    *object = static_cast<IRawElementProviderFragment *>(this);
  } else if (iid == IID_IRawElementProviderFragmentRoot) {
    *object = static_cast<IRawElementProviderFragmentRoot *>(this);
  } else if (iid == IID_ISelectionProvider) {
    *object = static_cast<ISelectionProvider *>(this);
  } else {
    return E_NOINTERFACE;
  }
  AddRef();
  return S_OK;
}

ULONG CandidateAutomationRoot::Release() {
  const ULONG remaining = --referenceCount_;
  if (remaining == 0) {
    delete this;
  }
  return remaining;
}

HRESULT CandidateAutomationRoot::get_ProviderOptions(ProviderOptions *options) {
  if (options == nullptr) {
    return E_POINTER;
  }
  *options = ProviderOptions_ServerSideProvider;
  return S_OK;
}

HRESULT CandidateAutomationRoot::GetPatternProvider(
    const PATTERNID patternId, IUnknown **provider) {
  if (provider == nullptr) {
    return E_POINTER;
  }
  *provider = nullptr;
  if (patternId == UIA_SelectionPatternId) {
    *provider = static_cast<ISelectionProvider *>(this);
    AddRef();
  }
  return S_OK;
}

HRESULT CandidateAutomationRoot::GetPropertyValue(const PROPERTYID propertyId,
                                                  VARIANT *value) {
  if (value == nullptr) {
    return E_POINTER;
  }
  VariantInit(value);
  if (propertyId == UIA_AutomationIdPropertyId) {
    return SetVariantString(value, L"IME_Candidate_Window");
  }
  if (propertyId == UIA_NamePropertyId) {
    return SetVariantString(value, L"Slime candidates");
  }
  if (propertyId == UIA_ControlTypePropertyId) {
    return SetVariantInt(value, UIA_ListControlTypeId);
  }
  if (propertyId == UIA_IsEnabledPropertyId ||
      propertyId == UIA_IsControlElementPropertyId ||
      propertyId == UIA_IsContentElementPropertyId) {
    return SetVariantBool(value, true);
  }
  if (propertyId == UIA_IsOffscreenPropertyId) {
    std::lock_guard lock(mutex_);
    return SetVariantBool(value, !visible_ || window_ == nullptr);
  }
  return S_OK;
}

HRESULT CandidateAutomationRoot::get_HostRawElementProvider(
    IRawElementProviderSimple **provider) {
  if (provider == nullptr) {
    return E_POINTER;
  }
  *provider = nullptr;
  HWND window = nullptr;
  {
    std::lock_guard lock(mutex_);
    window = window_;
  }
  return window != nullptr ? UiaHostProviderFromHwnd(window, provider)
                           : kElementNotAvailable;
}

HRESULT CandidateAutomationRoot::Navigate(
    const NavigateDirection direction, IRawElementProviderFragment **provider) {
  if (provider == nullptr) {
    return E_POINTER;
  }
  *provider = nullptr;
  std::size_t start = 0;
  std::size_t end = 0;
  const HRESULT result = VisibleRange(start, end);
  if (FAILED(result)) {
    return result;
  }
  if (start == end) {
    return S_OK;
  }
  if (direction == NavigateDirection_FirstChild) {
    return CreateItem(start, provider);
  }
  if (direction == NavigateDirection_LastChild) {
    return CreateItem(end - 1, provider);
  }
  return S_OK;
}

HRESULT CandidateAutomationRoot::GetRuntimeId(SAFEARRAY **runtimeId) {
  if (runtimeId == nullptr) {
    return E_POINTER;
  }
  *runtimeId = nullptr;
  return S_OK;
}

HRESULT CandidateAutomationRoot::get_BoundingRectangle(UiaRect *rectangle) {
  if (rectangle == nullptr) {
    return E_POINTER;
  }
  *rectangle = {};
  HWND window = nullptr;
  bool visible = false;
  {
    std::lock_guard lock(mutex_);
    window = window_;
    visible = visible_;
  }
  RECT bounds{};
  if (!visible || window == nullptr || !GetWindowRect(window, &bounds)) {
    return kElementNotAvailable;
  }
  rectangle->left = static_cast<double>(bounds.left);
  rectangle->top = static_cast<double>(bounds.top);
  rectangle->width = static_cast<double>(bounds.right - bounds.left);
  rectangle->height = static_cast<double>(bounds.bottom - bounds.top);
  return S_OK;
}

HRESULT CandidateAutomationRoot::GetEmbeddedFragmentRoots(SAFEARRAY **roots) {
  if (roots == nullptr) {
    return E_POINTER;
  }
  *roots = nullptr;
  return S_OK;
}

HRESULT CandidateAutomationRoot::SetFocus() { return kOperationNotSupported; }

HRESULT CandidateAutomationRoot::get_FragmentRoot(
    IRawElementProviderFragmentRoot **root) {
  if (root == nullptr) {
    return E_POINTER;
  }
  *root = static_cast<IRawElementProviderFragmentRoot *>(this);
  AddRef();
  return S_OK;
}

HRESULT CandidateAutomationRoot::ElementProviderFromPoint(
    const double x, const double y, IRawElementProviderFragment **provider) {
  if (provider == nullptr) {
    return E_POINTER;
  }
  *provider = nullptr;
  HWND window = nullptr;
  std::size_t start = 0;
  std::size_t end = 0;
  UINT rowHeight = 0;
  {
    std::lock_guard lock(mutex_);
    if (!visible_ || window_ == nullptr) {
      return kElementNotAvailable;
    }
    window = window_;
    start = pageStart_;
    end = std::min(pageStart_ + kSlimeCandidatePageSize, candidates_.size());
    rowHeight = rowHeight_;
  }
  RECT bounds{};
  if (rowHeight == 0 || !GetWindowRect(window, &bounds) || x < bounds.left ||
      x >= bounds.right || y < bounds.top || y >= bounds.bottom) {
    return S_OK;
  }
  const std::size_t row =
      static_cast<std::size_t>(y - static_cast<double>(bounds.top)) / rowHeight;
  const std::size_t index = start + row;
  return index < end ? CreateItem(index, provider) : S_OK;
}

HRESULT CandidateAutomationRoot::GetFocus(
    IRawElementProviderFragment **provider) {
  if (provider == nullptr) {
    return E_POINTER;
  }
  *provider = nullptr;
  std::size_t selected = 0;
  {
    std::lock_guard lock(mutex_);
    if (!visible_ || selected_ >= candidates_.size()) {
      return S_OK;
    }
    selected = selected_;
  }
  return CreateItem(selected, provider);
}

HRESULT CandidateAutomationRoot::GetSelection(SAFEARRAY **selection) {
  if (selection == nullptr) {
    return E_POINTER;
  }
  *selection = nullptr;
  std::size_t selected = 0;
  {
    std::lock_guard lock(mutex_);
    if (!visible_ || selected_ >= candidates_.size()) {
      *selection = SafeArrayCreateVector(VT_UNKNOWN, 0, 0);
      return *selection != nullptr ? S_OK : E_OUTOFMEMORY;
    }
    selected = selected_;
  }
  IRawElementProviderSimple *item = nullptr;
  HRESULT result = CreateSimpleItem(selected, &item);
  if (FAILED(result)) {
    return result;
  }
  SAFEARRAY *values = SafeArrayCreateVector(VT_UNKNOWN, 0, 1);
  if (values == nullptr) {
    item->Release();
    return E_OUTOFMEMORY;
  }
  LONG index = 0;
  result = SafeArrayPutElement(values, &index, item);
  item->Release();
  if (FAILED(result)) {
    SafeArrayDestroy(values);
    return result;
  }
  *selection = values;
  return S_OK;
}

HRESULT CandidateAutomationRoot::get_CanSelectMultiple(BOOL *multiple) {
  if (multiple == nullptr) {
    return E_POINTER;
  }
  *multiple = FALSE;
  return S_OK;
}

HRESULT CandidateAutomationRoot::get_IsSelectionRequired(BOOL *required) {
  if (required == nullptr) {
    return E_POINTER;
  }
  *required = TRUE;
  return S_OK;
}

void CandidateAutomationRoot::Update(
    HWND window, const std::vector<CandidatePresentation> &candidates,
    const std::size_t selected, const std::size_t pageStart,
    const UINT rowHeight, const int contentWidth) noexcept {
  try {
    std::lock_guard lock(mutex_);
    window_ = window;
    candidates_.clear();
    candidates_.reserve(candidates.size());
    for (const auto &candidate : candidates) {
      candidates_.push_back(candidate.accessibleName);
    }
    selected_ =
        candidates_.empty() ? 0 : std::min(selected, candidates_.size() - 1);
    pageStart_ = pageStart;
    rowHeight_ = rowHeight;
    contentWidth_ = contentWidth;
  } catch (...) {
    Disconnect();
  }
}

void CandidateAutomationRoot::SetVisible(const bool visible) noexcept {
  std::lock_guard lock(mutex_);
  visible_ = visible;
}

void CandidateAutomationRoot::Disconnect() noexcept {
  std::lock_guard lock(mutex_);
  window_ = nullptr;
  candidates_.clear();
  selected_ = 0;
  pageStart_ = 0;
  rowHeight_ = 0;
  contentWidth_ = 0;
  visible_ = false;
}

void CandidateAutomationRoot::RaiseOpened() noexcept {
  UiaRaiseAutomationEvent(static_cast<IRawElementProviderSimple *>(this),
                          UIA_MenuOpenedEventId);
}

void CandidateAutomationRoot::RaiseClosed() noexcept {
  UiaRaiseAutomationEvent(static_cast<IRawElementProviderSimple *>(this),
                          UIA_MenuClosedEventId);
}

void CandidateAutomationRoot::RaiseSelectionChanged() noexcept {
  std::size_t selected = 0;
  {
    std::lock_guard lock(mutex_);
    if (!visible_ || selected_ >= candidates_.size()) {
      return;
    }
    selected = selected_;
  }
  IRawElementProviderSimple *item = nullptr;
  if (SUCCEEDED(CreateSimpleItem(selected, &item))) {
    UiaRaiseAutomationEvent(item, UIA_SelectionItem_ElementSelectedEventId);
    item->Release();
  }
}

HRESULT CandidateAutomationRoot::Select(const std::size_t index) noexcept {
  HWND window = nullptr;
  {
    std::lock_guard lock(mutex_);
    const std::size_t end =
        std::min(pageStart_ + kSlimeCandidatePageSize, candidates_.size());
    if (!visible_ || window_ == nullptr || index < pageStart_ || index >= end) {
      return kElementNotAvailable;
    }
    window = window_;
  }
  if (index > std::numeric_limits<WPARAM>::max()) {
    return E_INVALIDARG;
  }
  return PostMessageW(window, kAutomationSelectMessage,
                      static_cast<WPARAM>(index), 0)
             ? S_OK
             : HRESULT_FROM_WIN32(GetLastError());
}

HRESULT CandidateAutomationRoot::CandidateName(const std::size_t index,
                                               std::wstring &name) noexcept {
  try {
    std::lock_guard lock(mutex_);
    if (window_ == nullptr || index >= candidates_.size()) {
      return kElementNotAvailable;
    }
    name = candidates_[index];
    return S_OK;
  } catch (...) {
    return E_OUTOFMEMORY;
  }
}

HRESULT CandidateAutomationRoot::IsSelected(const std::size_t index,
                                            BOOL *selected) noexcept {
  if (selected == nullptr) {
    return E_POINTER;
  }
  std::lock_guard lock(mutex_);
  if (window_ == nullptr || index >= candidates_.size()) {
    *selected = FALSE;
    return kElementNotAvailable;
  }
  *selected = index == selected_ ? TRUE : FALSE;
  return S_OK;
}

HRESULT CandidateAutomationRoot::CandidateRectangle(
    const std::size_t index, UiaRect *rectangle) noexcept {
  if (rectangle == nullptr) {
    return E_POINTER;
  }
  *rectangle = {};
  HWND window = nullptr;
  std::size_t pageStart = 0;
  std::size_t pageEnd = 0;
  UINT rowHeight = 0;
  int contentWidth = 0;
  {
    std::lock_guard lock(mutex_);
    pageStart = pageStart_;
    pageEnd = std::min(pageStart_ + kSlimeCandidatePageSize, candidates_.size());
    if (!visible_ || window_ == nullptr || index < pageStart ||
        index >= pageEnd) {
      return kElementNotAvailable;
    }
    window = window_;
    rowHeight = rowHeight_;
    contentWidth = contentWidth_;
  }
  RECT bounds{};
  if (rowHeight == 0 || !GetWindowRect(window, &bounds)) {
    return kElementNotAvailable;
  }
  const double top = static_cast<double>(
      bounds.top + static_cast<LONG>(index - pageStart) * rowHeight);
  rectangle->left = static_cast<double>(bounds.left);
  rectangle->top = top;
  rectangle->width = static_cast<double>(contentWidth);
  rectangle->height = static_cast<double>(rowHeight);
  return S_OK;
}

HRESULT CandidateAutomationRoot::VisibleRange(std::size_t &start,
                                              std::size_t &end) noexcept {
  std::lock_guard lock(mutex_);
  if (window_ == nullptr) {
    return kElementNotAvailable;
  }
  start = pageStart_;
  end = std::min(pageStart_ + kSlimeCandidatePageSize, candidates_.size());
  return S_OK;
}

HRESULT CandidateAutomationRoot::CreateItem(
    const std::size_t index, IRawElementProviderFragment **provider) noexcept {
  if (provider == nullptr) {
    return E_POINTER;
  }
  *provider = nullptr;
  std::size_t start = 0;
  std::size_t end = 0;
  const HRESULT result = VisibleRange(start, end);
  if (FAILED(result) || index < start || index >= end) {
    return FAILED(result) ? result : E_INVALIDARG;
  }
  auto *item = new (std::nothrow) CandidateAutomationItem(this, index);
  if (item == nullptr) {
    return E_OUTOFMEMORY;
  }
  *provider = static_cast<IRawElementProviderFragment *>(item);
  return S_OK;
}

HRESULT CandidateAutomationRoot::CreateSimpleItem(
    const std::size_t index, IRawElementProviderSimple **provider) noexcept {
  if (provider == nullptr) {
    return E_POINTER;
  }
  *provider = nullptr;
  std::size_t start = 0;
  std::size_t end = 0;
  const HRESULT result = VisibleRange(start, end);
  if (FAILED(result) || index < start || index >= end) {
    return FAILED(result) ? result : E_INVALIDARG;
  }
  auto *item = new (std::nothrow) CandidateAutomationItem(this, index);
  if (item == nullptr) {
    return E_OUTOFMEMORY;
  }
  *provider = static_cast<IRawElementProviderSimple *>(item);
  return S_OK;
}

long CandidateAutomationObjectCount() noexcept {
  return g_candidateAutomationObjectCount.load();
}

CandidateWindow::CandidateWindow(HINSTANCE instance, void *callbackContext,
                                 SelectionCallback callback) noexcept
    : instance_(instance), callbackContext_(callbackContext),
      callback_(callback),
      automationProvider_(new (std::nothrow) CandidateAutomationRoot()) {}

CandidateWindow::~CandidateWindow() {
  Hide();
  if (automationProvider_ != nullptr) {
    automationProvider_->Disconnect();
    UiaDisconnectProvider(
        static_cast<IRawElementProviderSimple *>(automationProvider_));
    automationProvider_->Release();
    automationProvider_ = nullptr;
  }
  if (window_ != nullptr) {
    DestroyWindow(window_);
  }
  if (font_ != nullptr && ownsFont_) {
    DeleteObject(font_);
  }
}

bool CandidateWindow::EnsureWindow(HWND owner) noexcept {
  if (owner == nullptr) {
    owner = GetFocus();
  }
  if (owner == nullptr) {
    owner = GetForegroundWindow();
  }
  owner = owner == nullptr ? nullptr : GetAncestor(owner, GA_ROOT);
  if (window_ != nullptr) {
    if (owner_ != owner) {
      SetWindowLongPtrW(window_, GWLP_HWNDPARENT,
                        reinterpret_cast<LONG_PTR>(owner));
      owner_ = owner;
    }
    return true;
  }
  if (!RegisterCandidateWindowClass(instance_)) {
    return false;
  }
  owner_ = owner;
  window_ = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                            kWindowClassName, L"Slime candidates",
                            WS_POPUP | WS_BORDER, 0, 0, 0, 0, owner_, nullptr,
                            instance_, this);
  if (window_ == nullptr) {
    return false;
  }
  if (automationProvider_ == nullptr) {
    automationProvider_ = new (std::nothrow) CandidateAutomationRoot();
  }
  RecreateFont();
  return true;
}

bool CandidateWindow::Update(HWND owner, const RECT &anchor,
                             const std::vector<CandidatePresentation> &candidates,
                             const std::size_t selected) noexcept {
  try {
    if (candidates.empty()) {
      Hide();
      return false;
    }
    if (!EnsureWindow(owner)) {
      return false;
    }
    const std::size_t previousSelected = selected_;
    const std::wstring previousSelection =
        selected_ < candidates_.size() ? candidates_[selected_].accessibleName
                                       : std::wstring{};
    candidates_ = candidates;
    selected_ = std::min(selected, candidates_.size() - 1);
    pageStart_ =
        (selected_ / kSlimeCandidatePageSize) * kSlimeCandidatePageSize;

    HDC device = GetDC(window_);
    if (device == nullptr) {
      Hide();
      return false;
    }
    const HGDIOBJ oldFont = SelectObject(device, font_);
    TEXTMETRICW metrics{};
    GetTextMetricsW(device, &metrics);
    rowHeight_ = static_cast<UINT>(std::max<LONG>(metrics.tmHeight, 1) +
                                   kVerticalPadding * 2);
    contentWidth_ = kMinimumContentWidth;
    const std::size_t pageEnd =
        std::min(pageStart_ + kSlimeCandidatePageSize, candidates_.size());
    for (std::size_t index = pageStart_; index < pageEnd; ++index) {
      const auto prefix = CandidatePrefix(index, pageStart_);
      SIZE prefixExtent{};
      SIZE candidateExtent{};
      SIZE annotationExtent{};
      if (GetTextExtentPoint32W(device, prefix.data(), 3, &prefixExtent) &&
          GetTextExtentPoint32W(
              device, candidates_[index].value.data(),
              static_cast<int>(candidates_[index].value.size()),
              &candidateExtent) &&
          GetTextExtentPoint32W(
              device, candidates_[index].annotation.data(),
              static_cast<int>(candidates_[index].annotation.size()),
              &annotationExtent)) {
        const int annotationWidth = candidates_[index].annotation.empty()
                                        ? 0
                                        : kAnnotationGap + annotationExtent.cx;
        contentWidth_ =
            std::max(contentWidth_,
                     static_cast<int>(prefixExtent.cx + candidateExtent.cx) +
                         annotationWidth +
                         kHorizontalPadding * 2);
      }
    }
    if (oldFont != nullptr) {
      SelectObject(device, oldFont);
    }
    ReleaseDC(window_, device);

    const auto rowCount = static_cast<int>(pageEnd - pageStart_);
    contentHeight_ = rowCount * static_cast<int>(rowHeight_);
    UpdateAutomation();
    Position(anchor);
    if (visible_ && automationProvider_ != nullptr &&
        (previousSelected != selected_ ||
         previousSelection != candidates_[selected_].accessibleName)) {
      automationProvider_->RaiseSelectionChanged();
    }
    InvalidateRect(window_, nullptr, FALSE);
    return true;
  } catch (...) {
    Hide();
    return false;
  }
}

void CandidateWindow::Hide() noexcept {
  if (window_ != nullptr) {
    ShowWindow(window_, SW_HIDE);
    if (std::exchange(visible_, false)) {
      NotifyWinEvent(EVENT_OBJECT_IME_HIDE, window_, OBJID_CLIENT,
                     CHILDID_SELF);
      if (automationProvider_ != nullptr) {
        automationProvider_->SetVisible(false);
        automationProvider_->RaiseClosed();
      }
    }
  }
  candidates_.clear();
  selected_ = 0;
  pageStart_ = 0;
  UpdateAutomation();
}

void CandidateWindow::UpdateAutomation() noexcept {
  if (automationProvider_ != nullptr) {
    automationProvider_->Update(window_, candidates_, selected_, pageStart_,
                                rowHeight_, contentWidth_);
  }
}

void CandidateWindow::RecreateFont() noexcept {
  if (font_ != nullptr && ownsFont_) {
    DeleteObject(font_);
  }
  font_ = nullptr;
  ownsFont_ = false;
  const UINT dpi = window_ == nullptr ? 96 : GetDpiForWindow(window_);
  font_ = CreateFontW(-MulDiv(10, static_cast<int>(dpi), 72), 0, 0, 0,
                      FW_NORMAL, FALSE, FALSE, FALSE, DEFAULT_CHARSET,
                      OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                      DEFAULT_PITCH | FF_DONTCARE, L"Yu Gothic UI");
  ownsFont_ = font_ != nullptr;
  if (font_ == nullptr) {
    font_ = static_cast<HFONT>(GetStockObject(DEFAULT_GUI_FONT));
  }
}

void CandidateWindow::Position(const RECT &anchor) noexcept {
  RECT windowRect{0, 0, contentWidth_, contentHeight_};
  AdjustWindowRectEx(&windowRect, WS_POPUP | WS_BORDER, FALSE,
                     WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST);
  const int width = windowRect.right - windowRect.left;
  const int height = windowRect.bottom - windowRect.top;

  HMONITOR monitor = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
  MONITORINFO monitorInfo{};
  monitorInfo.cbSize = sizeof(monitorInfo);
  if (!GetMonitorInfoW(monitor, &monitorInfo)) {
    monitorInfo.rcWork = RECT{0, 0, GetSystemMetrics(SM_CXSCREEN),
                              GetSystemMetrics(SM_CYSCREEN)};
  }
  int x = ClampCoordinate(anchor.left, monitorInfo.rcWork.left,
                          monitorInfo.rcWork.right - width);
  int y = anchor.bottom;
  if (y + height > monitorInfo.rcWork.bottom) {
    y = anchor.top - height;
  }
  y = ClampCoordinate(y, monitorInfo.rcWork.top,
                      monitorInfo.rcWork.bottom - height);
  RECT previous{};
  const bool hadPreviousRect = visible_ && GetWindowRect(window_, &previous);
  if (!SetWindowPos(window_, HWND_TOPMOST, x, y, width, height,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW)) {
    Hide();
    return;
  }
  if (!visible_) {
    NotifyWinEvent(EVENT_OBJECT_IME_SHOW, window_, OBJID_CLIENT, CHILDID_SELF);
    if (automationProvider_ != nullptr) {
      automationProvider_->SetVisible(true);
      automationProvider_->RaiseOpened();
    }
  } else if (!hadPreviousRect || previous.left != x || previous.top != y ||
             previous.right - previous.left != width ||
             previous.bottom - previous.top != height) {
    NotifyWinEvent(EVENT_OBJECT_IME_CHANGE, window_, OBJID_CLIENT,
                   CHILDID_SELF);
  }
  visible_ = true;
}

void CandidateWindow::Paint() noexcept {
  PAINTSTRUCT paint{};
  HDC device = BeginPaint(window_, &paint);
  if (device == nullptr) {
    return;
  }
  const HGDIOBJ oldFont = SelectObject(device, font_);
  SetBkMode(device, OPAQUE);
  const std::size_t pageEnd =
      std::min(pageStart_ + kSlimeCandidatePageSize, candidates_.size());
  for (std::size_t index = pageStart_; index < pageEnd; ++index) {
    const int row = static_cast<int>(index - pageStart_);
    RECT rowRect{0, row * static_cast<int>(rowHeight_), contentWidth_,
                 (row + 1) * static_cast<int>(rowHeight_)};
    const bool selected = index == selected_;
    SetBkColor(device, GetSysColor(selected ? COLOR_HIGHLIGHT : COLOR_WINDOW));
    SetTextColor(device,
                 GetSysColor(selected ? COLOR_HIGHLIGHTTEXT : COLOR_WINDOWTEXT));
    ExtTextOutW(device, kHorizontalPadding,
                rowRect.top + kVerticalPadding, ETO_OPAQUE, &rowRect, nullptr, 0,
                nullptr);
    const auto prefix = CandidatePrefix(index, pageStart_);
    SIZE prefixExtent{};
    TextOutW(device, kHorizontalPadding, rowRect.top + kVerticalPadding,
             prefix.data(), 3);
    GetTextExtentPoint32W(device, prefix.data(), 3, &prefixExtent);
    TextOutW(device, kHorizontalPadding + prefixExtent.cx,
             rowRect.top + kVerticalPadding, candidates_[index].value.data(),
             static_cast<int>(candidates_[index].value.size()));
    if (!candidates_[index].annotation.empty()) {
      SIZE annotationExtent{};
      if (GetTextExtentPoint32W(
              device, candidates_[index].annotation.data(),
              static_cast<int>(candidates_[index].annotation.size()),
              &annotationExtent)) {
        if (!selected) {
          SetTextColor(device, GetSysColor(COLOR_GRAYTEXT));
        }
        TextOutW(device,
                 contentWidth_ - kHorizontalPadding - annotationExtent.cx,
                 rowRect.top + kVerticalPadding,
                 candidates_[index].annotation.data(),
                 static_cast<int>(candidates_[index].annotation.size()));
      }
    }
  }
  if (oldFont != nullptr) {
    SelectObject(device, oldFont);
  }
  EndPaint(window_, &paint);
}

void CandidateWindow::SelectRowFromPoint(const LPARAM lParam,
                                         const bool accept) noexcept {
  if (rowHeight_ == 0 || candidates_.empty()) {
    return;
  }
  const int y = GET_Y_LPARAM(lParam);
  if (y < 0) {
    return;
  }
  const std::size_t row = static_cast<std::size_t>(y) / rowHeight_;
  const std::size_t index = pageStart_ + row;
  if (index >= candidates_.size() || row >= kSlimeCandidatePageSize) {
    return;
  }
  selected_ = index;
  UpdateAutomation();
  if (automationProvider_ != nullptr) {
    automationProvider_->RaiseSelectionChanged();
  }
  InvalidateRect(window_, nullptr, FALSE);
  if (callback_ != nullptr) {
    callback_(callbackContext_, static_cast<UINT>(index), accept);
  }
}

LRESULT CALLBACK CandidateWindow::WindowProcedure(const HWND window,
                                                   const UINT message,
                                                   const WPARAM wParam,
                                                   const LPARAM lParam) noexcept {
  CandidateWindow *instance = reinterpret_cast<CandidateWindow *>(
      GetWindowLongPtrW(window, GWLP_USERDATA));
  if (message == WM_NCCREATE) {
    const auto *create = reinterpret_cast<const CREATESTRUCTW *>(lParam);
    instance = static_cast<CandidateWindow *>(create->lpCreateParams);
    SetWindowLongPtrW(window, GWLP_USERDATA,
                      reinterpret_cast<LONG_PTR>(instance));
    instance->window_ = window;
  }
  if (instance == nullptr) {
    return DefWindowProcW(window, message, wParam, lParam);
  }
  try {
    return instance->HandleMessage(message, wParam, lParam);
  } catch (...) {
    instance->Hide();
    return DefWindowProcW(window, message, wParam, lParam);
  }
}

LRESULT CandidateWindow::HandleMessage(const UINT message, const WPARAM wParam,
                                       const LPARAM lParam) noexcept {
  switch (message) {
  case WM_GETOBJECT:
    if (lParam == static_cast<LPARAM>(UiaRootObjectId) &&
        automationProvider_ != nullptr) {
      return UiaReturnRawElementProvider(
          window_, wParam, lParam,
          static_cast<IRawElementProviderSimple *>(automationProvider_));
    }
    return DefWindowProcW(window_, message, wParam, lParam);
  case kAutomationSelectMessage: {
    const std::size_t index = static_cast<std::size_t>(wParam);
    if (index < candidates_.size() && callback_ != nullptr) {
      callback_(callbackContext_, static_cast<UINT>(index), false);
    }
    return 0;
  }
  case WM_MOUSEACTIVATE:
    return MA_NOACTIVATE;
  case WM_LBUTTONDOWN:
    SelectRowFromPoint(lParam, false);
    return 0;
  case WM_LBUTTONDBLCLK:
    SelectRowFromPoint(lParam, true);
    return 0;
  case WM_PAINT:
    Paint();
    return 0;
  case WM_ERASEBKGND:
    return 1;
  case WM_DPICHANGED: {
    RecreateFont();
    const auto *suggested = reinterpret_cast<const RECT *>(lParam);
    SetWindowPos(window_, nullptr, suggested->left, suggested->top,
                 suggested->right - suggested->left,
                 suggested->bottom - suggested->top,
                 SWP_NOACTIVATE | SWP_NOZORDER);
    return 0;
  }
  case WM_NCDESTROY:
    visible_ = false;
    if (automationProvider_ != nullptr) {
      automationProvider_->Disconnect();
    }
    SetWindowLongPtrW(window_, GWLP_USERDATA, 0);
    {
      const HWND destroyed = window_;
      window_ = nullptr;
      return DefWindowProcW(destroyed, message, wParam, lParam);
    }
  default:
    return DefWindowProcW(window_, message, wParam, lParam);
  }
}
