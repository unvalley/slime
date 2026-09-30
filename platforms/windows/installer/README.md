# Windows installer

`Slime.nsi` builds an x86 NSIS bootstrapper for either x64 Windows or Windows
11 ARM. The x64 package contains x64 and x86 TSF payloads. The ARM package
contains the ARM64X/native payload and the x86 payload needed by 32-bit host
processes. Each package is per-machine and requires elevation because both COM
registry views and `%ProgramFiles%` are updated. It never downloads code while
installing and supports NSIS's case-sensitive `/S` silent mode.

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

The build script reads each PE header before invoking the packager. Every file
under `x64` must have machine `0x8664`, every file under `x86` must have machine
`0x014C`, and the resulting bootstrapper must also be `0x014C`. A mislabeled
payload is rejected before signing or installer construction.

After building the flat ARM64X artifact, build its unsigned development
installer with the same script:

```powershell
scripts/build-windows-installer.ps1 `
  -Version 0.1.0 `
  -PayloadARM64X target/windows-arm64x `
  -PayloadX86 target/windows-x86/Release `
  -Output target/package/Slime-0.1.0-windows-arm64-unsigned.exe
```

The ARM64X allowlist and forwarder exports are verified before packaging. The
installer refuses to run unless Windows reports native ARM64, installs the
64-bit payload under `arm64x`, registers its forwarder, and keeps the x86 TSF
registration for 32-bit applications. When upgrading an older ARM machine that
used the legacy `x64` native directory, rollback can restore that registration
and a successful versioned update removes the old product-owned payload.

The public Windows workflow also builds older development versions from the
same payloads and runs real silent install, failed-update rollback, versioned
update, and uninstall sequences on disposable x64 and native ARM64 hosted
runners. The smoke test checks payload hashes, both COM
registry views, the Japanese language profile, removal of the previous version,
both installed settings executables' isolated save/load/change-notification
self-test, and preservation of local user data. It also replaces the x86 service
DLL in an intermediate fixture with a non-service PE, proving that a failure
after x64 registration restores the old x64/x86 registration, removes the
partial new directory, and preserves user data. Every artifact must remain
explicitly unsigned, so this is development evidence only and never satisfies
the signed release gate.

NSIS 3.12 is used in CI. NSIS and Modern UI are distributed under licenses
that permit commercial use. The public release boundary is stricter than this
development build:

| Artifact | Architectures | Signing order | Verification |
| --- | --- | --- | --- |
| `SlimeIME.dll` | ARM64X, ARM64, x64, x86 | Sign before packaging | Authenticode, COM load, TSF registration |
| `slime_ffi.dll` | ARM64X, ARM64, x64, x86 | Sign before packaging | Authenticode, dependency load |
| `SlimeIMERegister.exe` | ARM64, x64, x86 | Sign before packaging | install/uninstall rollback smoke |
| `SlimeSettings.exe` | ARM64, x64, x86 | Sign before packaging | launch, save, propagation smoke |
| `Slime-<version>-windows*.exe` | x86 bootstrapper restricted to its native OS | Sign after packaging | Authenticode, product version, source revision, hash, clean install/update/downgrade rejection/uninstall |

Signing credentials and certificate identifiers must remain external to the
repository. A successful unsigned CI package is only **artifact-ready for
development**. Microsoft requires a third-party IME and every distributed PE
to be signed; general distribution additionally requires a trusted code-signing
chain and a clean-machine consumer test.

The ARM64 workflow is still development evidence. Its unsigned installer and
native lifecycle do not satisfy the release gate until the remote workflow has
passed for the exact revision, every ARM64X/native/x64/x86 PE and embedded
uninstaller has been signed, and the signed package has completed consumer
input and accessibility checks on a clean Windows 11 ARM machine.

For a release candidate, sign and timestamp every payload file first. The
installer build also needs an external `.cmd` or `.exe` signer that
accepts exactly one path; NSIS invokes it while constructing `Uninstall.exe`.
The signer, credentials, timestamp endpoint, and certificate selection remain
outside this repository. `-Release` accepts only the repository root of a clean
Git checkout and embeds its 40-character `HEAD` in the installer metadata and
uninstall registry entry.

```powershell
scripts/build-windows-installer.ps1 `
  -Release `
  -Version 0.1.0 `
  -PayloadX64 target/package/x64 `
  -PayloadX86 target/package/x86 `
  -UninstallerSigner C:\release-private\sign-one-file.cmd `
  -ExpectedSignerThumbprint 0123456789ABCDEF0123456789ABCDEF01234567 `
  -Output target/package/Slime-0.1.0-windows-unsigned-outer.exe
```

For Windows 11 ARM use `-PayloadARM64X target/package/arm64x` instead of
`-PayloadX64`; the same build gate then requires valid signatures on all eight
ARM64X/native implementation files and all four x86 files.

This command rejects unsigned, untrusted, untimestamped, mixed-signer,
unexpected-signer, non-RSA, RSA keys below 2048 bits, and wrong-EKU payloads.
The expected certificate thumbprint is release configuration and must remain
outside the repository. The signer certificate must contain the code-signing
EKU, and the timestamp certificate must contain the time-stamping EKU. It produces
an outer installer that must then be signed and
timestamped by the commercial release pipeline. The VM lifecycle test rejects
the result if the embedded uninstaller was not actually signed.

After signing every PE and the outer installer, verify architecture,
Authenticode trust, timestamping, and a single signer:

```powershell
$revision = (git rev-parse HEAD).Trim()
scripts/verify-windows-release.ps1 `
  -Version 0.1.0 `
  -PayloadX64 target/package/x64 `
  -PayloadX86 target/package/x86 `
  -ExpectedSourceRevision $revision `
  -ExpectedSignerThumbprint 0123456789ABCDEF0123456789ABCDEF01234567 `
  -Installer target/package/Slime-0.1.0-windows.exe
```

The same verifier accepts `-PayloadARM64X` in place of `-PayloadX64`. It first
checks the ARM64X forwarder structure and exports, then requires valid
code-signing and timestamp EKUs and one expected signer across the forwarders,
ARM64/x64 implementations, native tools, x86 payload, and outer installer.

The consumer lifecycle gate must run from an elevated shell in a disposable,
clean VM matching the installer architecture. `SLIME_RELEASE_TEST_VM=1` is an
intentional safety latch.
Passing a previous signed installer also exercises versioned update, removal
of the old payload, and rejection of that older installer after the update.
The previous installer must use the current expected signer unless an approved
rotation is declared with `-PreviousExpectedSignerThumbprint`.
The current installer is then run a second time to prove that a same-version
install leaves the installed manifest and user data unchanged.
The test verifies the exact installed file and directory
allowlist, absence of reparse points, both COM registry views, the Japanese
language profile, uninstall registry values, Start Menu targets, installed
signatures, both settings executables' isolated save/load/change-notification
self-test, uninstall cleanup, and preservation of local user data.
It emits architecture, version, source revision, signer thumbprint, installer
SHA-256, and the canonical installed-manifest SHA-256 as one canonical JSON
binding so the later interactive gate can require an exact string match.

```powershell
$env:SLIME_RELEASE_TEST_VM = "1"
$revision = (git rev-parse HEAD).Trim()
scripts/test-windows-install-lifecycle.ps1 `
  -Version 0.1.0 `
  -PayloadX64 C:\release\x64 `
  -PayloadX86 C:\release\x86 `
  -ExpectedSourceRevision $revision `
  -ExpectedSignerThumbprint 0123456789ABCDEF0123456789ABCDEF01234567 `
  -PreviousInstaller C:\release\Slime-0.0.9-windows.exe `
  -Installer C:\release\Slime-0.1.0-windows.exe
```

On a native Windows 11 ARM VM, pass `-PayloadARM64X C:\release\arm64x` instead
of `-PayloadX64`. The same gate verifies the exact 12-file ARM64X/x86 payload,
native and WOW6432 COM registrations, embedded uninstaller signature, settings
self-tests, signed update, downgrade rejection, cleanup, and user-data preservation.

The lifecycle gate deliberately does not claim that interactive input or
accessibility worked. After reinstalling the same signed release in a clean
interactive session, use `scripts/windows-consumer-input-gate.ps1`. It binds
each manual observation to the expected signer, product version, source revision, installer
SHA-256, and the complete installed-file manifest. See
`docs/windows-consumer-verification.md` for the required order and commands.
