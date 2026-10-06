param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [string]$PayloadX64,

    [string]$PayloadARM64X,

    [Parameter(Mandatory = $true)]
    [string]$PayloadX86,

    [Parameter(Mandatory = $true)]
    [string]$Output,

    [string]$Makensis = "makensis.exe",

    [switch]$Release,

    [string]$UninstallerSigner,

    [ValidatePattern('^[0-9A-Fa-f]{40}$')]
    [string]$ExpectedSignerThumbprint
)

$ErrorActionPreference = "Stop"
$makensisCommand = Get-Command $Makensis -ErrorAction SilentlyContinue
if ($makensisCommand) {
    $Makensis = $makensisCommand.Source
} elseif ($Makensis -eq "makensis.exe") {
    $installedMakensis = Join-Path ${env:ProgramFiles(x86)} "NSIS/makensis.exe"
    if (-not (Test-Path -LiteralPath $installedMakensis -PathType Leaf)) {
        throw "makensis.exe was not found on PATH or in the standard NSIS installation"
    }
    $Makensis = $installedMakensis
} else {
    throw "makensis executable was not found: $Makensis"
}
. (Join-Path $PSScriptRoot "windows-signature.ps1")
. (Join-Path $PSScriptRoot "windows-install-layout.ps1")
. (Join-Path $PSScriptRoot "windows-release-source.ps1")
$repository = Split-Path -Parent $PSScriptRoot
$installerSource = Join-Path $repository "platforms/windows/installer/Slime.nsi"
$hasX64 = -not [string]::IsNullOrWhiteSpace($PayloadX64)
$hasARM64X = -not [string]::IsNullOrWhiteSpace($PayloadARM64X)
if ($hasX64 -eq $hasARM64X) {
    throw "Specify exactly one of -PayloadX64 or -PayloadARM64X"
}
$nativeDirectory = if ($hasARM64X) { "arm64x" } else { "x64" }
$native = if ($hasARM64X) {
    (Resolve-Path $PayloadARM64X).Path
} else {
    (Resolve-Path $PayloadX64).Path
}
$x86 = (Resolve-Path $PayloadX86).Path
$outputDirectory = Split-Path -Parent $Output
if (-not $outputDirectory) {
    $outputDirectory = "."
}
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
$absoluteOutput = [System.IO.Path]::GetFullPath($Output)
$sourceRevision = if ($Release) {
    Get-SlimeCleanReleaseSourceRevision $repository
} else {
    "development"
}

$requiredFiles = @(
    "SlimeIME.dll",
    "slime_ffi.dll",
    "SlimeIMERegister.exe",
    "SlimeSettings.exe"
)
$nativeFiles = Get-SlimeNativePayloadFiles $nativeDirectory
if ($hasARM64X) {
    $arm64xVerification = @{
        Artifact = $native
        AllowSigned = $Release
    }
    if ($Release) {
        $arm64xVerification.ExpectedSourceRevision = $sourceRevision
        $arm64xVerification.ExpectedVersion = $Version
    }
    & (Join-Path $PSScriptRoot "verify-windows-arm64x-artifact.ps1") `
        @arm64xVerification | Out-Null
}
$payloadSigners = @()
$nativeMachine = if ($hasARM64X) { $null } else { [UInt16]0x8664 }
foreach ($architecture in @(
    @{ Path = $native; Machine = $nativeMachine; Files = $nativeFiles },
    @{ Path = $x86; Machine = [UInt16]0x014C; Files = $requiredFiles }
)) {
    foreach ($file in $architecture.Files) {
        $candidate = Join-Path $architecture.Path $file
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "Missing installer payload: $candidate"
        }
        if ($null -ne $architecture.Machine) {
            Assert-SlimePeMachine $candidate $architecture.Machine
        }
        if ($Release) {
            $signature = Assert-SlimeAuthenticodeSignature $candidate
            $payloadSigners += $signature.SignerCertificate.Thumbprint
            Assert-SlimePeSourceRevision $candidate $sourceRevision
            Assert-SlimePeProductVersion $candidate $Version
        }
    }
}

$makensisArguments = @(
    "-WX",
    # The script is UTF-8 without a BOM; makensis otherwise reads it in the
    # ANSI code page and installs mojibake Start Menu names.
    "/INPUTCHARSET", "UTF8",
    "/DVERSION=$Version",
    "/DVERSION_QUAD=$Version.0",
    "/DSOURCE_REVISION=$sourceRevision",
    "/DPAYLOAD_X86=$x86",
    "/DOUTPUT=$absoluteOutput"
)
if ($hasARM64X) {
    $makensisArguments += "/DARM64_INSTALLER=1"
    $makensisArguments += "/DPAYLOAD_ARM64X=$native"
} else {
    $makensisArguments += "/DPAYLOAD_X64=$native"
}
if ($Release) {
    if (-not $UninstallerSigner) {
        throw "Release builds require -UninstallerSigner"
    }
    if (-not $ExpectedSignerThumbprint) {
        throw "Release builds require -ExpectedSignerThumbprint"
    }
    $resolvedSigner = (Resolve-Path $UninstallerSigner).Path
    if (-not (Test-Path -LiteralPath $resolvedSigner -PathType Leaf)) {
        throw "Uninstaller signer is not a file: $resolvedSigner"
    }
    if ([System.IO.Path]::GetExtension($resolvedSigner) -notin @(".cmd", ".exe")) {
        throw "Uninstaller signer must be an external .cmd or .exe accepting one file path"
    }
    if ($resolvedSigner.Contains('"') -or $resolvedSigner.Contains("`r") -or $resolvedSigner.Contains("`n")) {
        throw "Uninstaller signer path contains unsupported characters"
    }
    $distinctPayloadSigners = @($payloadSigners | Sort-Object -Unique)
    if ($distinctPayloadSigners.Count -ne 1) {
        throw "Release payloads were signed by different certificates"
    }
    Assert-SlimeSignerThumbprint `
        $distinctPayloadSigners[0] `
        $ExpectedSignerThumbprint `
        "release payloads"
    $makensisArguments += "/DUNINSTALLER_SIGNER=$resolvedSigner"
} elseif ($UninstallerSigner -or $ExpectedSignerThumbprint) {
    throw "-UninstallerSigner and -ExpectedSignerThumbprint are valid only with -Release"
}

& $Makensis @makensisArguments $installerSource
if ($LASTEXITCODE -ne 0) {
    throw "makensis failed with exit code $LASTEXITCODE"
}
if (-not (Test-Path -LiteralPath $absoluteOutput -PathType Leaf)) {
    throw "Installer was not produced: $absoluteOutput"
}
Assert-SlimePeMachine $absoluteOutput 0x014C
$embeddedSourceRevision = (Get-Item -LiteralPath $absoluteOutput).VersionInfo.Comments
if ($embeddedSourceRevision -ne "Source revision: $sourceRevision") {
    throw "Installer source revision metadata does not match the build input"
}

$signature = Get-AuthenticodeSignature -LiteralPath $absoluteOutput
if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::NotSigned) {
    throw "Outer installer must be unsigned before the final signing stage; status: $($signature.Status)"
}
Get-FileHash -Algorithm SHA256 -LiteralPath $absoluteOutput
if ($Release) {
    Write-Output "Built $nativeDirectory release candidate from $sourceRevision with signed payloads and embedded uninstaller"
} else {
    Write-Output "Built unsigned $nativeDirectory development installer"
}
