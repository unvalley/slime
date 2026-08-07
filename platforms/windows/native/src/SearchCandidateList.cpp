#include "SearchCandidateList.h"

#include <algorithm>
#include <atomic>
#include <limits>
#include <utility>

namespace {

std::atomic_long g_searchCandidateObjectCount = 0;

class SearchCandidateString final : public ITfCandidateString {
public:
  SearchCandidateString(std::wstring value, const ULONG index)
      : value_(std::move(value)), index_(index) {
    ++g_searchCandidateObjectCount;
  }

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **object) override {
    if (object == nullptr) {
      return E_POINTER;
    }
    *object = nullptr;
    if (iid != IID_IUnknown && iid != IID_ITfCandidateString) {
      return E_NOINTERFACE;
    }
    *object = static_cast<ITfCandidateString *>(this);
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

  HRESULT STDMETHODCALLTYPE GetString(BSTR *value) override {
    if (value == nullptr) {
      return E_POINTER;
    }
    *value = nullptr;
    if (value_.size() > std::numeric_limits<UINT>::max()) {
      return E_FAIL;
    }
    *value = SysAllocStringLen(value_.data(), static_cast<UINT>(value_.size()));
    return *value != nullptr ? S_OK : E_OUTOFMEMORY;
  }

  HRESULT STDMETHODCALLTYPE GetIndex(ULONG *index) override {
    if (index == nullptr) {
      return E_POINTER;
    }
    *index = index_;
    return S_OK;
  }

private:
  ~SearchCandidateString() { --g_searchCandidateObjectCount; }

  std::atomic_ulong referenceCount_{1};
  std::wstring value_;
  ULONG index_ = 0;
};

HRESULT CreateCandidateString(const std::vector<std::wstring> &candidates,
                              const std::size_t index,
                              ITfCandidateString **candidate) noexcept {
  if (candidate == nullptr) {
    return E_POINTER;
  }
  *candidate = nullptr;
  if (index >= candidates.size() ||
      index > std::numeric_limits<ULONG>::max()) {
    return E_INVALIDARG;
  }
  try {
    *candidate = new SearchCandidateString(candidates[index],
                                           static_cast<ULONG>(index));
    return S_OK;
  } catch (...) {
    return E_OUTOFMEMORY;
  }
}

class SearchCandidateEnumerator final : public IEnumTfCandidates {
public:
  explicit SearchCandidateEnumerator(std::vector<std::wstring> candidates,
                                     const std::size_t position = 0)
      : candidates_(std::move(candidates)), position_(position) {
    ++g_searchCandidateObjectCount;
  }

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **object) override {
    if (object == nullptr) {
      return E_POINTER;
    }
    *object = nullptr;
    if (iid != IID_IUnknown && iid != IID_IEnumTfCandidates) {
      return E_NOINTERFACE;
    }
    *object = static_cast<IEnumTfCandidates *>(this);
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

  HRESULT STDMETHODCALLTYPE Clone(IEnumTfCandidates **enumerator) override {
    if (enumerator == nullptr) {
      return E_POINTER;
    }
    *enumerator = nullptr;
    try {
      *enumerator = new SearchCandidateEnumerator(candidates_, position_);
      return S_OK;
    } catch (...) {
      return E_OUTOFMEMORY;
    }
  }

  HRESULT STDMETHODCALLTYPE Next(const ULONG count,
                                 ITfCandidateString **candidates,
                                 ULONG *fetched) override {
    if (count > 0 && candidates == nullptr) {
      return E_INVALIDARG;
    }
    if (fetched != nullptr) {
      *fetched = 0;
    }
    ULONG produced = 0;
    const std::size_t originalPosition = position_;
    while (produced < count && originalPosition + produced < candidates_.size()) {
      candidates[produced] = nullptr;
      const HRESULT result = CreateCandidateString(
          candidates_, originalPosition + produced, &candidates[produced]);
      if (FAILED(result)) {
        for (ULONG index = 0; index < produced; ++index) {
          candidates[index]->Release();
          candidates[index] = nullptr;
        }
        return result;
      }
      ++produced;
    }
    position_ = originalPosition + produced;
    if (fetched != nullptr) {
      *fetched = produced;
    }
    return produced == count ? S_OK : S_FALSE;
  }

  HRESULT STDMETHODCALLTYPE Reset() override {
    position_ = 0;
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE Skip(const ULONG count) override {
    const std::size_t remaining = candidates_.size() - position_;
    const std::size_t skipped = std::min<std::size_t>(count, remaining);
    position_ += skipped;
    return skipped == count ? S_OK : S_FALSE;
  }

private:
  ~SearchCandidateEnumerator() { --g_searchCandidateObjectCount; }

  std::atomic_ulong referenceCount_{1};
  std::vector<std::wstring> candidates_;
  std::size_t position_ = 0;
};

class SearchCandidateList final : public ITfCandidateList {
public:
  explicit SearchCandidateList(std::vector<std::wstring> candidates)
      : candidates_(std::move(candidates)) {
    ++g_searchCandidateObjectCount;
  }

  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void **object) override {
    if (object == nullptr) {
      return E_POINTER;
    }
    *object = nullptr;
    if (iid != IID_IUnknown && iid != IID_ITfCandidateList) {
      return E_NOINTERFACE;
    }
    *object = static_cast<ITfCandidateList *>(this);
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

  HRESULT STDMETHODCALLTYPE EnumCandidates(
      IEnumTfCandidates **enumerator) override {
    if (enumerator == nullptr) {
      return E_POINTER;
    }
    *enumerator = nullptr;
    try {
      *enumerator = new SearchCandidateEnumerator(candidates_);
      return S_OK;
    } catch (...) {
      return E_OUTOFMEMORY;
    }
  }

  HRESULT STDMETHODCALLTYPE GetCandidate(
      const ULONG index, ITfCandidateString **candidate) override {
    return CreateCandidateString(candidates_, index, candidate);
  }

  HRESULT STDMETHODCALLTYPE GetCandidateNum(ULONG *count) override {
    if (count == nullptr) {
      return E_POINTER;
    }
    if (candidates_.size() > std::numeric_limits<ULONG>::max()) {
      return E_FAIL;
    }
    *count = static_cast<ULONG>(candidates_.size());
    return S_OK;
  }

  HRESULT STDMETHODCALLTYPE SetResult(const ULONG index,
                                      const TfCandidateResult result) override {
    if (result == CAND_CANCELED) {
      return S_OK;
    }
    if (result != CAND_SELECTED && result != CAND_FINALIZED) {
      return E_INVALIDARG;
    }
    return index < candidates_.size() ? S_OK : E_INVALIDARG;
  }

private:
  ~SearchCandidateList() { --g_searchCandidateObjectCount; }

  std::atomic_ulong referenceCount_{1};
  std::vector<std::wstring> candidates_;
};

} // namespace

HRESULT CreateSearchCandidateList(std::vector<std::wstring> candidates,
                                  ITfCandidateList **list) noexcept {
  if (list == nullptr) {
    return E_POINTER;
  }
  *list = nullptr;
  try {
    *list = new SearchCandidateList(std::move(candidates));
    return S_OK;
  } catch (...) {
    return E_OUTOFMEMORY;
  }
}

std::vector<std::wstring>
FilterSearchCandidates(std::vector<std::wstring> candidates,
                       const std::size_t limit) {
  std::vector<std::wstring> filtered;
  if (limit == 0) {
    return filtered;
  }
  filtered.reserve(std::min(candidates.size(), limit));
  for (auto &candidate : candidates) {
    const bool overlaps =
        std::any_of(filtered.begin(), filtered.end(),
                    [&candidate](const std::wstring &existing) {
                      return candidate.starts_with(existing) ||
                             existing.starts_with(candidate);
                    });
    if (candidate.empty() || overlaps) {
      continue;
    }
    filtered.push_back(std::move(candidate));
    if (filtered.size() == limit) {
      break;
    }
  }
  return filtered;
}

long SearchCandidateObjectCount() noexcept {
  return g_searchCandidateObjectCount.load();
}
