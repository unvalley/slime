# Windows ARM64X development artifact

This directory defines the export contracts used to assemble a 64-bit TSF
development artifact that loads in both x64 and native ARM64 client processes.
The public C ABI and COM implementations remain separate ordinary DLLs; small
ARM64X pure forwarders select the matching implementation at load time.

The flat artifact contains:

| File | Purpose |
| --- | --- |
| `SlimeIME.dll` | ARM64X COM forwarder registered with TSF |
| `SlimeIME_x64.dll` | x64 COM implementation |
| `SlimeIME_arm64.dll` | native ARM64 COM implementation |
| `slime_ffi.dll` | ARM64X C ABI forwarder |
| `slime_ffi_x64.dll` | x64 Rust implementation |
| `slime_ffi_arm64.dll` | native ARM64 Rust implementation |
| `SlimeIMERegister.exe` | native ARM64 registration and load-probe helper |
| `SlimeSettings.exe` | native ARM64 settings executable |

`scripts/build-windows-arm64x.ps1` builds and verifies this allowlist from the
ordinary x64 and ARM64 workflow artifacts. The workflow then loads the same COM
forwarder once from a native ARM64 helper and once from an emulated x64 helper.
Neither probe changes registry state.

The public workflow packages this artifact together with the x86 payload in an
ARM64-aware unsigned installer. On a native Windows 11 ARM runner it exercises
clean install, rejected-update rollback, versioned update, settings self-tests,
uninstall, exact layout and hash checks, registration cleanup, and user-data
preservation. This remains development evidence until the workflow passes for
the exact revision and every PE is signed and tested in clean consumer sessions
with real-app input and accessibility coverage.
