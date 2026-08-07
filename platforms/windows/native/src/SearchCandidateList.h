#pragma once

#include <windows.h>

#include <cstddef>
#include <string>
#include <vector>

#include "TsfCandidateCompat.h"

HRESULT CreateSearchCandidateList(std::vector<std::wstring> candidates,
                                  ITfCandidateList **list) noexcept;

std::vector<std::wstring>
FilterSearchCandidates(std::vector<std::wstring> candidates,
                       std::size_t limit);

long SearchCandidateObjectCount() noexcept;
