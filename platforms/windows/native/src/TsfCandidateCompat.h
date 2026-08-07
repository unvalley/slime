#pragma once

#include <msctf.h>
#if __has_include(<ctffunc.h>)
#include <ctffunc.h>
#endif

// MinGW's msctf.h omits ITfTextInputProcessorEx even though the activation
// flags and the base processor are present. This guarded declaration mirrors
// the Windows SDK ABI so cross-platform syntax checks exercise the same class
// layout used by MSVC.
#ifndef __ITfTextInputProcessorEx_INTERFACE_DEFINED__
#define __ITfTextInputProcessorEx_INTERFACE_DEFINED__

inline constexpr GUID IID_ITfTextInputProcessorEx = {
    0x6e4e2102,
    0xf9cd,
    0x433d,
    {0xb4, 0x96, 0x30, 0x3c, 0xe0, 0x3a, 0x65, 0x07}};

MIDL_INTERFACE("6e4e2102-f9cd-433d-b496-303ce03a6507")
ITfTextInputProcessorEx : public ITfTextInputProcessor {
public:
  virtual HRESULT STDMETHODCALLTYPE ActivateEx(ITfThreadMgr *threadManager,
                                               TfClientId clientId,
                                               DWORD flags) = 0;
};

#endif

#ifndef __ITfCandidateString_INTERFACE_DEFINED__
#define __ITfCandidateString_INTERFACE_DEFINED__

inline constexpr GUID IID_ITfCandidateString = {
    0x581f317e,
    0xfd9d,
    0x443f,
    {0xb9, 0x72, 0xed, 0x00, 0x46, 0x7c, 0x5d, 0x40}};

MIDL_INTERFACE("581f317e-fd9d-443f-b972-ed00467c5d40")
ITfCandidateString : public IUnknown {
public:
  virtual HRESULT STDMETHODCALLTYPE GetString(BSTR *value) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetIndex(ULONG *index) = 0;
};

#endif

#ifndef __IEnumTfCandidates_INTERFACE_DEFINED__
#define __IEnumTfCandidates_INTERFACE_DEFINED__

inline constexpr GUID IID_IEnumTfCandidates = {
    0xdefb1926,
    0x6c80,
    0x4ce8,
    {0x87, 0xd4, 0xd6, 0xb7, 0x2b, 0x81, 0x2b, 0xde}};

MIDL_INTERFACE("defb1926-6c80-4ce8-87d4-d6b72b812bde")
IEnumTfCandidates : public IUnknown {
public:
  virtual HRESULT STDMETHODCALLTYPE Clone(IEnumTfCandidates **enumerator) = 0;
  virtual HRESULT STDMETHODCALLTYPE Next(ULONG count,
                                         ITfCandidateString **candidates,
                                         ULONG *fetched) = 0;
  virtual HRESULT STDMETHODCALLTYPE Reset() = 0;
  virtual HRESULT STDMETHODCALLTYPE Skip(ULONG count) = 0;
};

#endif

#ifndef __ITfCandidateList_INTERFACE_DEFINED__
#define __ITfCandidateList_INTERFACE_DEFINED__

enum TfCandidateResult {
  CAND_FINALIZED = 0,
  CAND_SELECTED = 0x1,
  CAND_CANCELED = 0x2,
};

inline constexpr GUID IID_ITfCandidateList = {
    0xa3ad50fb,
    0x9bdb,
    0x49e3,
    {0xa8, 0x43, 0x6c, 0x76, 0x52, 0x0f, 0xbf, 0x5d}};

MIDL_INTERFACE("a3ad50fb-9bdb-49e3-a843-6c76520fbf5d")
ITfCandidateList : public IUnknown {
public:
  virtual HRESULT STDMETHODCALLTYPE EnumCandidates(
      IEnumTfCandidates **enumerator) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetCandidate(
      ULONG index, ITfCandidateString **candidate) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetCandidateNum(ULONG *count) = 0;
  virtual HRESULT STDMETHODCALLTYPE SetResult(ULONG index,
                                               TfCandidateResult result) = 0;
};

#endif

#ifndef __ITfFunction_INTERFACE_DEFINED__
#define __ITfFunction_INTERFACE_DEFINED__

inline constexpr GUID IID_ITfFunction = {
    0xdb593490,
    0x098f,
    0x11d3,
    {0x8d, 0xf0, 0x00, 0x10, 0x5a, 0x27, 0x99, 0xb5}};

MIDL_INTERFACE("db593490-098f-11d3-8df0-00105a2799b5")
ITfFunction : public IUnknown {
public:
  virtual HRESULT STDMETHODCALLTYPE GetDisplayName(BSTR *name) = 0;
};

#endif

#ifndef __ITfFnConfigure_INTERFACE_DEFINED__
#define __ITfFnConfigure_INTERFACE_DEFINED__

inline constexpr GUID IID_ITfFnConfigure = {
    0x88f567c6,
    0x1757,
    0x49f8,
    {0xa1, 0xb2, 0x89, 0x23, 0x4c, 0x1e, 0xef, 0xf9}};

MIDL_INTERFACE("88f567c6-1757-49f8-a1b2-89234c1eeff9")
ITfFnConfigure : public ITfFunction {
public:
  virtual HRESULT STDMETHODCALLTYPE Show(HWND parent, LANGID language,
                                         REFGUID profile) = 0;
};

#endif

#ifndef __ITfFnSearchCandidateProvider_INTERFACE_DEFINED__
#define __ITfFnSearchCandidateProvider_INTERFACE_DEFINED__

inline constexpr GUID IID_ITfFnSearchCandidateProvider = {
    0x87a2ad8f,
    0xf27b,
    0x4920,
    {0x85, 0x01, 0x67, 0x60, 0x22, 0x80, 0x17, 0x5d}};

MIDL_INTERFACE("87a2ad8f-f27b-4920-8501-67602280175d")
ITfFnSearchCandidateProvider : public ITfFunction {
public:
  virtual HRESULT STDMETHODCALLTYPE GetSearchCandidates(
      BSTR query, BSTR applicationId, ITfCandidateList **list) = 0;
  virtual HRESULT STDMETHODCALLTYPE SetResult(BSTR query, BSTR applicationId,
                                               BSTR result) = 0;
};

#endif

// MinGW's msctf.h currently omits the TSF candidate UI interface that is
// present in the Windows SDK. Keep the ABI declaration local and guarded so
// the native shell can be syntax-checked from macOS without shadowing the
// authoritative Windows SDK definition in MSVC builds.
#ifndef __ITfCandidateListUIElement_INTERFACE_DEFINED__
#define __ITfCandidateListUIElement_INTERFACE_DEFINED__

inline constexpr GUID IID_ITfCandidateListUIElement = {
    0xea1ea138,
    0x19df,
    0x11d7,
    {0xa6, 0xd2, 0x00, 0x06, 0x5b, 0x84, 0x43, 0x5c}};

inline constexpr DWORD TF_CLUIE_DOCUMENTMGR = 0x00000001;
inline constexpr DWORD TF_CLUIE_COUNT = 0x00000002;
inline constexpr DWORD TF_CLUIE_SELECTION = 0x00000004;
inline constexpr DWORD TF_CLUIE_STRING = 0x00000008;
inline constexpr DWORD TF_CLUIE_PAGEINDEX = 0x00000010;
inline constexpr DWORD TF_CLUIE_CURRENTPAGE = 0x00000020;

MIDL_INTERFACE("ea1ea138-19df-11d7-a6d2-00065b84435c")
ITfCandidateListUIElement : public ITfUIElement {
public:
  virtual HRESULT STDMETHODCALLTYPE GetUpdatedFlags(DWORD *flags) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetDocumentMgr(ITfDocumentMgr **documentManager) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetCount(UINT *count) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetSelection(UINT *index) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetString(UINT index, BSTR *value) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetPageIndex(UINT *indexes, UINT size, UINT *pageCount) = 0;
  virtual HRESULT STDMETHODCALLTYPE SetPageIndex(UINT *indexes, UINT pageCount) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetCurrentPage(UINT *page) = 0;
};

#endif

#ifndef __ITfCandidateListUIElementBehavior_INTERFACE_DEFINED__
#define __ITfCandidateListUIElementBehavior_INTERFACE_DEFINED__

inline constexpr GUID IID_ITfCandidateListUIElementBehavior = {
    0x85fad185,
    0x58ce,
    0x497a,
    {0x94, 0x60, 0x35, 0x53, 0x66, 0xb6, 0x4b, 0x9a}};

MIDL_INTERFACE("85fad185-58ce-497a-9460-355366b64b9a")
ITfCandidateListUIElementBehavior : public ITfCandidateListUIElement {
public:
  virtual HRESULT STDMETHODCALLTYPE SetSelection(UINT index) = 0;
  virtual HRESULT STDMETHODCALLTYPE Finalize() = 0;
  virtual HRESULT STDMETHODCALLTYPE Abort() = 0;
};

#endif

#ifndef __ITfIntegratableCandidateListUIElement_INTERFACE_DEFINED__
#define __ITfIntegratableCandidateListUIElement_INTERFACE_DEFINED__

inline constexpr GUID IID_ITfIntegratableCandidateListUIElement = {
    0xc7a6f54f,
    0xb180,
    0x416f,
    {0xb2, 0xbf, 0x7b, 0xf2, 0xe4, 0x68, 0x3d, 0x7b}};

enum TfIntegratableCandidateListSelectionStyle {
  STYLE_ACTIVE_SELECTION = 0,
  STYLE_IMPLIED_SELECTION = 0x1,
};

MIDL_INTERFACE("c7a6f54f-b180-416f-b2bf-7bf2e4683d7b")
ITfIntegratableCandidateListUIElement : public IUnknown {
public:
  virtual HRESULT STDMETHODCALLTYPE SetIntegrationStyle(GUID style) = 0;
  virtual HRESULT STDMETHODCALLTYPE GetSelectionStyle(
      TfIntegratableCandidateListSelectionStyle *style) = 0;
  virtual HRESULT STDMETHODCALLTYPE OnKeyDown(WPARAM wParam, LPARAM lParam,
                                               BOOL *eaten) = 0;
  virtual HRESULT STDMETHODCALLTYPE ShowCandidateNumbers(BOOL *show) = 0;
  virtual HRESULT STDMETHODCALLTYPE FinalizeExactCompositionString() = 0;
};

#endif
