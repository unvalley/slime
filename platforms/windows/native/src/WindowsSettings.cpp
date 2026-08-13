#include "WindowsPreferences.h"

#include <windows.h>

#include <array>
#include <string>

namespace {

constexpr wchar_t kSettingsWindowClass[] = L"SlimeIME.SettingsWindow";
constexpr int kWindowWidth = 640;
constexpr int kWindowHeight = 752;

enum ControlId : int {
  kLiveConversion = 100,
  kTypoCorrection,
  kHistoryCompletion,
  kHistoryLearning,
  kDictionaryTechnology,
  kDictionaryBusiness,
  kDictionaryCreative,
  kDateShortNumeric,
  kDateIsoNumeric,
  kDateMonthDayWeekday,
  kDateLongGregorian,
  kDateLongReiwa,
  kDateShortReiwa,
  kDateWeekday,
  kSave,
  kRestoreDefaults,
  kStatus,
};

struct SettingsWindowState {
  WindowsPreferences preferences;
  HFONT font = nullptr;
  HWND status = nullptr;
};

int Scale(const int value, const UINT dpi) noexcept {
  return MulDiv(value, static_cast<int>(dpi), 96);
}

void SetControlFont(const HWND control, const HFONT font) noexcept {
  SendMessageW(control, WM_SETFONT, reinterpret_cast<WPARAM>(font), TRUE);
}

HWND AddControl(const HWND parent, const wchar_t *className,
                const wchar_t *text, const DWORD style, const int x,
                const int y, const int width, const int height, const int id,
                const UINT dpi, const HFONT font) noexcept {
  HWND control = CreateWindowExW(
      0, className, text, WS_CHILD | WS_VISIBLE | style, Scale(x, dpi),
      Scale(y, dpi), Scale(width, dpi), Scale(height, dpi), parent,
      reinterpret_cast<HMENU>(static_cast<INT_PTR>(id)), GetModuleHandleW(nullptr),
      nullptr);
  if (control != nullptr) {
    SetControlFont(control, font);
  }
  return control;
}

void SetChecked(const HWND window, const int id, const bool checked) noexcept {
  CheckDlgButton(window, id, checked ? BST_CHECKED : BST_UNCHECKED);
}

bool IsChecked(const HWND window, const int id) noexcept {
  return IsDlgButtonChecked(window, id) == BST_CHECKED;
}

void PresentPreferences(const HWND window,
                        const WindowsPreferences &preferences) noexcept {
  SetChecked(window, kLiveConversion, preferences.liveConversion);
  SetChecked(window, kTypoCorrection, preferences.typoCorrectionEnabled);
  SetChecked(window, kHistoryCompletion, preferences.historyCompletion);
  SetChecked(window, kHistoryLearning, preferences.historyLearning);
  SetChecked(window, kDictionaryTechnology,
             (preferences.dictionaryPacks & (1U << 0)) != 0);
  SetChecked(window, kDictionaryBusiness,
             (preferences.dictionaryPacks & (1U << 1)) != 0);
  SetChecked(window, kDictionaryCreative,
             (preferences.dictionaryPacks & (1U << 2)) != 0);
  SetChecked(window, kDateShortNumeric,
             (preferences.dateFormatMask & (1U << 0)) != 0);
  SetChecked(window, kDateIsoNumeric,
             (preferences.dateFormatMask & (1U << 1)) != 0);
  SetChecked(window, kDateMonthDayWeekday,
             (preferences.dateFormatMask & (1U << 2)) != 0);
  SetChecked(window, kDateLongGregorian,
             (preferences.dateFormatMask & (1U << 3)) != 0);
  SetChecked(window, kDateLongReiwa,
             (preferences.dateFormatMask & (1U << 4)) != 0);
  SetChecked(window, kDateShortReiwa,
             (preferences.dateFormatMask & (1U << 5)) != 0);
  SetChecked(window, kDateWeekday,
             (preferences.dateFormatMask & (1U << 6)) != 0);
}

WindowsPreferences ReadPreferences(const HWND window) noexcept {
  WindowsPreferences preferences;
  preferences.liveConversion = IsChecked(window, kLiveConversion);
  preferences.typoCorrectionEnabled = IsChecked(window, kTypoCorrection);
  preferences.historyCompletion = IsChecked(window, kHistoryCompletion);
  preferences.historyLearning = IsChecked(window, kHistoryLearning);
  preferences.dictionaryPacks =
      (IsChecked(window, kDictionaryTechnology) ? 1U << 0 : 0) |
      (IsChecked(window, kDictionaryBusiness) ? 1U << 1 : 0) |
      (IsChecked(window, kDictionaryCreative) ? 1U << 2 : 0);
  preferences.dateFormatMask =
      (IsChecked(window, kDateShortNumeric) ? 1U << 0 : 0) |
      (IsChecked(window, kDateIsoNumeric) ? 1U << 1 : 0) |
      (IsChecked(window, kDateMonthDayWeekday) ? 1U << 2 : 0) |
      (IsChecked(window, kDateLongGregorian) ? 1U << 3 : 0) |
      (IsChecked(window, kDateLongReiwa) ? 1U << 4 : 0) |
      (IsChecked(window, kDateShortReiwa) ? 1U << 5 : 0) |
      (IsChecked(window, kDateWeekday) ? 1U << 6 : 0);
  return preferences;
}

std::wstring ErrorMessage(const DWORD error) noexcept {
  try {
    wchar_t *message = nullptr;
    const DWORD length = FormatMessageW(
        FORMAT_MESSAGE_ALLOCATE_BUFFER | FORMAT_MESSAGE_FROM_SYSTEM |
            FORMAT_MESSAGE_IGNORE_INSERTS,
        nullptr, error, 0, reinterpret_cast<wchar_t *>(&message), 0, nullptr);
    if (length == 0 || message == nullptr) {
      return L"設定ファイルを保存できませんでした。";
    }
    std::wstring result(message, length);
    LocalFree(message);
    while (!result.empty() &&
           (result.back() == L'\r' || result.back() == L'\n')) {
      result.pop_back();
    }
    return result;
  } catch (...) {
    return L"設定ファイルを保存できませんでした。";
  }
}

void Save(const HWND window, SettingsWindowState &state) noexcept {
  const std::wstring path = WindowsPreferencesPath();
  const WindowsPreferences preferences = ReadPreferences(window);
  const DWORD error = SaveWindowsPreferences(path, preferences);
  if (error == ERROR_SUCCESS) {
    state.preferences = preferences;
    SetWindowTextW(
        state.status,
        L"保存しました。入力中の文字列がない状態で、1秒以内に反映されます。");
    return;
  }
  const std::wstring message = ErrorMessage(error);
  MessageBoxW(window, message.c_str(), L"Slime - 保存できません",
              MB_OK | MB_ICONERROR);
}

void CreateSettingsControls(const HWND window,
                            SettingsWindowState &state) noexcept {
  const UINT dpi = GetDpiForWindow(window);
  state.font = CreateFontW(-Scale(10, dpi), 0, 0, 0, FW_NORMAL, FALSE, FALSE,
                           FALSE, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
                           CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                           DEFAULT_PITCH | FF_DONTCARE, L"Segoe UI");
  if (state.font == nullptr) {
    state.font = static_cast<HFONT>(GetStockObject(DEFAULT_GUI_FONT));
  }

  AddControl(window, L"STATIC", L"Slime 設定", SS_LEFT, 24, 18, 300, 26,
             0, dpi, state.font);
  AddControl(window, L"STATIC",
             L"設定はすべてこのPC内に保存され、入力内容は外部に送信されません。",
             SS_LEFT, 24, 48, 580, 24, 0, dpi, state.font);

  AddControl(window, L"BUTTON", L"変換", BS_GROUPBOX, 20, 82, 590, 106, 0,
             dpi, state.font);
  AddControl(window, L"BUTTON", L"ライブ変換", BS_AUTOCHECKBOX | WS_TABSTOP,
             38, 110, 220, 28, kLiveConversion, dpi, state.font);
  AddControl(window, L"BUTTON", L"入力ミスの訂正候補を表示",
             BS_AUTOCHECKBOX | WS_TABSTOP, 38, 142, 300, 28,
             kTypoCorrection, dpi, state.font);

  AddControl(window, L"BUTTON", L"入力履歴", BS_GROUPBOX, 20, 198, 590, 102,
             0, dpi, state.font);
  AddControl(window, L"BUTTON", L"入力履歴を変換候補に使用",
             BS_AUTOCHECKBOX | WS_TABSTOP, 38, 226, 300, 28,
             kHistoryCompletion, dpi, state.font);
  AddControl(window, L"BUTTON", L"新しい確定結果を学習",
             BS_AUTOCHECKBOX | WS_TABSTOP, 38, 258, 300, 28,
             kHistoryLearning, dpi, state.font);

  AddControl(window, L"BUTTON", L"分野別辞書", BS_GROUPBOX, 20, 310, 590,
             132, 0, dpi, state.font);
  AddControl(window, L"BUTTON", L"テクノロジー", BS_AUTOCHECKBOX | WS_TABSTOP,
             38, 338, 250, 28, kDictionaryTechnology, dpi, state.font);
  AddControl(window, L"BUTTON", L"ビジネス", BS_AUTOCHECKBOX | WS_TABSTOP, 38,
             370, 250, 28, kDictionaryBusiness, dpi, state.font);
  AddControl(window, L"BUTTON", L"クリエイティブ", BS_AUTOCHECKBOX | WS_TABSTOP,
             38, 402, 250, 28, kDictionaryCreative, dpi, state.font);

  AddControl(window, L"BUTTON", L"日付候補", BS_GROUPBOX, 20, 452, 590, 166,
             0, dpi, state.font);
  const std::array<std::pair<const wchar_t *, int>, 7> dateControls{{
      {L"月/日", kDateShortNumeric},
      {L"年/月/日", kDateIsoNumeric},
      {L"月日と曜日", kDateMonthDayWeekday},
      {L"西暦の年月日", kDateLongGregorian},
      {L"和暦の年月日", kDateLongReiwa},
      {L"和暦の短縮表記", kDateShortReiwa},
      {L"曜日", kDateWeekday},
  }};
  for (std::size_t index = 0; index < dateControls.size(); ++index) {
    const int column = static_cast<int>(index % 2);
    const int row = static_cast<int>(index / 2);
    AddControl(window, L"BUTTON", dateControls[index].first,
               BS_AUTOCHECKBOX | WS_TABSTOP, 38 + column * 280,
               480 + row * 32, 250, 28, dateControls[index].second, dpi,
               state.font);
  }

  AddControl(window, L"BUTTON", L"既定値に戻す", BS_PUSHBUTTON | WS_TABSTOP,
             310, 640, 130, 34, kRestoreDefaults, dpi, state.font);
  AddControl(window, L"BUTTON", L"設定を保存", BS_DEFPUSHBUTTON | WS_TABSTOP,
             450, 640, 150, 34, kSave, dpi, state.font);
  state.status = AddControl(window, L"STATIC", L"", SS_LEFT, 24, 686, 576,
                            34, kStatus, dpi, state.font);
  PresentPreferences(window, state.preferences);
}

LRESULT CALLBACK SettingsWindowProcedure(const HWND window, const UINT message,
                                         const WPARAM wParam,
                                         const LPARAM lParam) noexcept {
  auto *state = reinterpret_cast<SettingsWindowState *>(
      GetWindowLongPtrW(window, GWLP_USERDATA));
  if (message == WM_NCCREATE) {
    const auto *create = reinterpret_cast<const CREATESTRUCTW *>(lParam);
    state = static_cast<SettingsWindowState *>(create->lpCreateParams);
    SetWindowLongPtrW(window, GWLP_USERDATA,
                      reinterpret_cast<LONG_PTR>(state));
  }
  if (state == nullptr) {
    return DefWindowProcW(window, message, wParam, lParam);
  }
  switch (message) {
  case WM_CREATE:
    CreateSettingsControls(window, *state);
    return 0;
  case WM_COMMAND:
    if (HIWORD(wParam) == BN_CLICKED && LOWORD(wParam) == kSave) {
      Save(window, *state);
      return 0;
    }
    if (HIWORD(wParam) == BN_CLICKED &&
        LOWORD(wParam) == kRestoreDefaults) {
      state->preferences = WindowsPreferences{};
      PresentPreferences(window, state->preferences);
      SetWindowTextW(state->status,
                     L"既定値を表示しました。保存すると反映されます。");
      return 0;
    }
    break;
  case WM_DESTROY:
    if (state->font != nullptr &&
        state->font != GetStockObject(DEFAULT_GUI_FONT)) {
      DeleteObject(state->font);
    }
    state->font = nullptr;
    PostQuitMessage(0);
    return 0;
  default:
    break;
  }
  return DefWindowProcW(window, message, wParam, lParam);
}

} // namespace

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE, PWSTR, int showCommand) {
  SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);

  if (HWND existing = FindWindowW(kSettingsWindowClass, nullptr);
      existing != nullptr) {
    ShowWindow(existing, SW_RESTORE);
    SetForegroundWindow(existing);
    return 0;
  }

  SettingsWindowState state;
  const std::wstring path = WindowsPreferencesPath();
  const WindowsPreferencesLoadStatus loadStatus =
      LoadWindowsPreferences(path, state.preferences);
  if (loadStatus == WindowsPreferencesLoadStatus::notFound) {
    state.preferences = WindowsPreferences{};
  }

  WNDCLASSEXW windowClass{};
  windowClass.cbSize = sizeof(windowClass);
  windowClass.lpfnWndProc = SettingsWindowProcedure;
  windowClass.hInstance = instance;
  windowClass.hCursor = LoadCursorW(nullptr, IDC_ARROW);
  windowClass.hIcon = LoadIconW(nullptr, IDI_APPLICATION);
  windowClass.hbrBackground = reinterpret_cast<HBRUSH>(COLOR_WINDOW + 1);
  windowClass.lpszClassName = kSettingsWindowClass;
  if (RegisterClassExW(&windowClass) == 0) {
    return 1;
  }

  const UINT dpi = GetDpiForSystem();
  const int width = Scale(kWindowWidth, dpi);
  const int height = Scale(kWindowHeight, dpi);
  const int x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
  const int y = (GetSystemMetrics(SM_CYSCREEN) - height) / 2;
  HWND window = CreateWindowExW(
      0, kSettingsWindowClass, L"Slime 設定",
      WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX, x, y, width,
      height, nullptr, nullptr, instance, &state);
  if (window == nullptr) {
    return 1;
  }
  if (loadStatus == WindowsPreferencesLoadStatus::invalid) {
    SetWindowTextW(state.status,
                   L"設定ファイルが壊れているため、既定値を表示しています。");
  } else if (loadStatus == WindowsPreferencesLoadStatus::ioError) {
    SetWindowTextW(state.status,
                   L"設定ファイルを読み込めないため、現在の表示は既定値です。");
  }
  ShowWindow(window, showCommand == 0 ? SW_SHOWNORMAL : showCommand);
  UpdateWindow(window);

  MSG message{};
  while (GetMessageW(&message, nullptr, 0, 0) > 0) {
    if (!IsDialogMessageW(window, &message)) {
      TranslateMessage(&message);
      DispatchMessageW(&message);
    }
  }
  return static_cast<int>(message.wParam);
}
