#pragma once

#include <windows.h>

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

inline constexpr std::size_t kSlimeCandidatePageSize = 9;

struct CandidatePresentation {
  std::wstring value;
  std::wstring annotation;
  std::wstring accessibleName;
};

CandidatePresentation BuildCandidatePresentation(std::wstring value,
                                                 std::uint32_t annotation,
                                                 std::wstring detail);

class CandidateAutomationRoot;

long CandidateAutomationObjectCount() noexcept;

class CandidateWindow final {
public:
  using SelectionCallback = void (*)(void *context, UINT candidateIndex,
                                     bool accept) noexcept;

  CandidateWindow(HINSTANCE instance, void *callbackContext,
                  SelectionCallback callback) noexcept;
  ~CandidateWindow();

  CandidateWindow(const CandidateWindow &) = delete;
  CandidateWindow &operator=(const CandidateWindow &) = delete;

  bool Update(HWND owner, const RECT &anchor,
              const std::vector<CandidatePresentation> &candidates,
              std::size_t selected) noexcept;
  void Hide() noexcept;

  static LRESULT CALLBACK WindowProcedure(HWND window, UINT message,
                                          WPARAM wParam, LPARAM lParam) noexcept;

private:
  LRESULT HandleMessage(UINT message, WPARAM wParam, LPARAM lParam) noexcept;
  bool EnsureWindow(HWND owner) noexcept;
  void RecreateFont() noexcept;
  void Paint() noexcept;
  void SelectRowFromPoint(LPARAM lParam, bool accept) noexcept;
  void Position(const RECT &anchor) noexcept;
  void UpdateAutomation() noexcept;
  HINSTANCE instance_ = nullptr;
  HWND window_ = nullptr;
  HWND owner_ = nullptr;
  HFONT font_ = nullptr;
  bool ownsFont_ = false;
  void *callbackContext_ = nullptr;
  SelectionCallback callback_ = nullptr;
  std::vector<CandidatePresentation> candidates_;
  std::size_t selected_ = 0;
  std::size_t pageStart_ = 0;
  UINT rowHeight_ = 0;
  int contentWidth_ = 0;
  int contentHeight_ = 0;
  bool visible_ = false;
  CandidateAutomationRoot *automationProvider_ = nullptr;
};
