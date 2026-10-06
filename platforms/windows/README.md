# Slime for Windows

This directory is the Windows Text Services Framework (TSF) adapter boundary.
`native/` contains the in-process C++ COM shell based on Microsoft's TSF
contracts and calls the platform-independent Rust engine through the typed
callback C ABI. The same C ABI path has key-by-key regression coverage on every
development host. The Windows native test binary also drives the typed ABI on
x64, x86, and native ARM64, selects an annotated correction and a candidate
added after bounded recall expansion, and commits both through the
`AcceptCandidate` event used by TSF candidate consumers.

The Rust boundary also exports `slime_process_actions_v2`, which keeps the
existing callback ABI intact while separating a candidate's committed value,
legacy display string, semantic annotation, and optional detail. The Windows
adapter uses v2 without changing index-based selection: the desktop popup draws
the committed value and annotation separately, while UILess and UI Automation
consumers receive a combined accessible name. A real-system Narrator check is
still required before treating that source-level contract as release evidence.

The native shell currently implements `ITfTextInputProcessorEx` (including the
legacy `ITfTextInputProcessor` contract and activation-mode flags),
`ITfKeyEventSink`, `ITfCompositionSink`, synchronous read/write edit sessions,
composition updates/commit/cancel, COM/language-profile/category registration,
and an explicit profile-enablement helper. It does not use IMM32 or `SendInput`.

Candidate cycling updates the composition surface. The shell also publishes
candidate strings, selection, and page changes through
`ITfCandidateListUIElement` for TSF UILess consumers. When TSF asks the text
service to draw its own UI, a non-activating desktop popup follows the
composition, exposes nine candidates per page, accepts `1`–`9`, and supports
mouse selection and double-click acceptance. TSF can suppress that popup and
consume the same candidate data in UILess mode. The UILess element also exposes
selection, finalize, abort, Search-box integration style, and keyboard behavior
through `ITfCandidateListUIElementBehavior` and
`ITfIntegratableCandidateListUIElement`. `ITfFunctionProvider` also exposes an
`ITfFnSearchCandidateProvider` that obtains ranked conversions without changing
the active composition, removes redundant prefix-overlapping results, and feeds
accepted results back to local history. The native popup exposes a UI Automation
list with the required `IME_Candidate_Window` automation ID, candidate names,
single-selection state, programmatic selection, and menu/selection events for
Narrator. `SlimeSettings.exe` persists live conversion, history use/learning,
domain dictionaries, and date formats under `%LOCALAPPDATA%\\Slime`. Active text
services watch the directory asynchronously and apply a valid atomic settings
update when no composition is active; the key path only polls an event handle.
At composition start, the service compares the current TSF caret with its last
observation. After an external caret or client change, it reads only a bounded
prefix immediately before the selection inside the same synchronous edit
session and supplies it as transient left context. The core caps it again,
never persists it or learns an unknown reading from it, and secure activation
does not request the document text.
`ITfFnConfigure` exposes the settings launcher through TSF. Secure TSF sessions
also force the Rust engine into private mode. An offline NSIS development
installer packages both x64 and x86 payloads, performs registration rollback,
supports silent installation, and preserves local user data during uninstall.
Unsigned clean-Windows install/update/uninstall is covered for both the x64/x86
installer and the ARM64X/x86 installer in their matching hosted runners. Real
signing and real-app compatibility tests remain required before this is a
distributable Windows IME.

Windows builds embed `Source revision: <40-hex-commit>` and the three-part
product version in the version metadata of the Rust DLL, COM DLL, registration
helper, settings executable, and ARM64X forwarders. Release packaging and
verification reject any payload whose embedded revision or version differs
from the clean installer source, even when its signature is otherwise valid.
This prevents a signed artifact from another build from being silently mixed
into the release set.

Run `just check-windows` to type-check the Rust boundary for x64, x86, and
ARM64. The Windows workflow builds all three Rust DLLs and native COM DLLs with
MSVC, runs the native parser/file-monitor/COM-function tests, checks COM
exports, exercises typed candidate selection/acceptance against the Windows
Rust DLL, and uploads self-contained unsigned development artifacts. ARM64 runs
on a native runner instead of being treated as an emulated x64 result.

The workflow packages x64 and x86 artifacts into one unsigned offline
installer. It also combines the x64 and ARM64 implementations behind ARM64X COM
and C ABI forwarders, verifies the exact flat layout and exports, loads the
result from both native ARM64 and emulated x64 probes, then packages it with x86
and exercises the unsigned lifecycle on native Windows 11 ARM. See
`arm64x/README.md` and
`installer/README.md` for the artifact boundaries and release gates.

For development installation, place matching-bitness `SlimeIME.dll`,
`slime_ffi.dll`, and `SlimeSettings.exe` together, then run the same-bitness
helper from an elevated terminal. On x64 Windows, repeat this for the x64 and
x86 pairs so both host process types can load Slime:

```powershell
SlimeIMERegister.exe install C:\absolute\path\to\SlimeIME.dll
```

Run `SlimeSettings.exe` directly during development. Hosts that expose TSF's
configure action can open the same executable through `ITfFnConfigure`.

Use `uninstall` with the same path to disable the profile and remove its TSF and
COM registration. The helper enables Slime for the current user without making
it the default input method. A real Windows machine is still required to verify
Win+Space, composition, popup placement and mouse behavior, candidate UI,
UILess/Search mode, Narrator/UI Automation behavior, packaged apps,
settings propagation across packaged apps, install/update/uninstall, and
signing.
