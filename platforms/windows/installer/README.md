# Windows installer

`Slime.nsi` builds one x86 NSIS bootstrapper for x64 Windows containing both
x64 and x86 TSF payloads. The package is per-machine and requires elevation
because both COM registry views and `%ProgramFiles%` are updated. It never
downloads code while installing and supports NSIS's case-sensitive `/S` silent
mode.

The installer writes each release to a versioned directory, registers both
bitnesses through `SlimeIMERegister.exe`, and restores the preceding registered
version if either new registration fails. Uninstall performs the reverse
operation and leaves `%LOCALAPPDATA%\Slime` intact so settings, dictionaries,
and learned history are not silently destroyed.

Build an unsigned development installer on Windows after producing the two
native payload directories:

```powershell
scripts/build-windows-installer.ps1 `
  -Version 0.1.0 `
  -PayloadX64 target/windows-x64/Release `
  -PayloadX86 target/windows-x86/Release `
  -Output target/package/Slime-0.1.0-windows-unsigned.exe
```

NSIS 3.12 is used in CI. NSIS and Modern UI are distributed under licenses
that permit commercial use. The public release boundary is stricter than this
development build:

| Artifact | Architectures | Signing order | Verification |
| --- | --- | --- | --- |
| `SlimeIME.dll` | x64, x86 | Sign before packaging | Authenticode, COM load, TSF registration |
| `slime_ffi.dll` | x64, x86 | Sign before packaging | Authenticode, dependency load |
| `SlimeIMERegister.exe` | x64, x86 | Sign before packaging | install/uninstall rollback smoke |
| `SlimeSettings.exe` | x64, x86 | Sign before packaging | launch, save, propagation smoke |
| `Slime-<version>-windows.exe` | x86 bootstrapper for x64 Windows | Sign after packaging | Authenticode, hash, clean install/update/uninstall |

Signing credentials and certificate identifiers must remain external to the
repository. A successful unsigned CI package is only **artifact-ready for
development**. Microsoft requires a third-party IME and every distributed PE
to be signed; general distribution additionally requires a trusted code-signing
chain and a clean-machine consumer test.
