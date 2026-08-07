# Slime for Windows

This directory is the Windows Text Services Framework (TSF) adapter boundary.
`native/` contains the in-process C++ COM shell based on Microsoft's TSF
contracts and calls the platform-independent Rust engine through the typed
callback C ABI. The same C ABI path has key-by-key regression coverage on every
development host.

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
`ITfFnConfigure` exposes the settings launcher through TSF. Secure TSF sessions
also force the Rust engine into private mode. An offline NSIS development
installer packages both x64 and x86 payloads, performs registration rollback,
supports silent installation, and preserves local user data during uninstall.
Signing and clean-Windows install/update/uninstall and real-app compatibility
tests remain required before this is a distributable Windows IME.

Run `just check-windows` to type-check the Rust boundary for both x64 and x86.
The Windows workflow builds both Rust DLLs and both native COM DLLs with MSVC,
runs the native parser/file-monitor/COM-function tests, checks COM exports, and
uploads self-contained unsigned development artifacts.

The workflow also packages those two artifacts into one unsigned offline
installer. See `installer/README.md` for the signing order and release gates.

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
