param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [string]$PayloadX64,

    [string]$PayloadARM64X,

    [Parameter(Mandatory = $true)]
    [string]$PayloadX86,

    [Parameter(Mandatory = $true)]
    [string]$Installer,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9A-Fa-f]{40}$')]
    [string]$ExpectedSignerThumbprint,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$ExpectedSourceRevision
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-signature.ps1")
. (Join-Path $PSScriptRoot "windows-install-layout.ps1")
$requiredFiles = @(
    "SlimeIME.dll",
    "slime_ffi.dll",
    "SlimeIMERegister.exe",
    "SlimeSettings.exe"
)
$hasX64 = -not [string]::IsNullOrWhiteSpace($PayloadX64)
$hasARM64X = -not [string]::IsNullOrWhiteSpace($PayloadARM64X)
if ($hasX64 -eq $hasARM64X) {
    throw "Specify exactly one of -PayloadX64 or -PayloadARM64X"
}
$nativeDirectory = if ($hasARM64X) { "arm64x" } else { "x64" }
$nativeFiles = Get-SlimeNativePayloadSpecification $nativeDirectory

function Assert-SignedPe([string]$Path, [UInt16]$ExpectedMachine = 0) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Missing release artifact: $Path"
    }
    if ($ExpectedMachine -ne 0) {
        Assert-SlimePeMachine $Path $ExpectedMachine
    }
    $signature = Assert-SlimeAuthenticodeSignature $Path
    return [PSCustomObject]@{
        Path = $Path
        SignerThumbprint = $signature.SignerCertificate.Thumbprint
    }
}

$native = if ($hasARM64X) {
    (Resolve-Path $PayloadARM64X).Path
} else {
    (Resolve-Path $PayloadX64).Path
}
$x86 = (Resolve-Path $PayloadX86).Path
$installerPath = (Resolve-Path $Installer).Path
if ($hasARM64X) {
    & (Join-Path $PSScriptRoot "verify-windows-arm64x-artifact.ps1") `
        -Artifact $native `
        -ExpectedSourceRevision $ExpectedSourceRevision `
        -ExpectedVersion $Version `
        -AllowSigned | Out-Null
}
$artifacts = @()
foreach ($file in $nativeFiles) {
    $artifacts += Assert-SignedPe (Join-Path $native $file.Name) $file.Machine
}
foreach ($file in $requiredFiles) {
    $artifacts += Assert-SignedPe (Join-Path $x86 $file) 0x014C
}
$artifacts += Assert-SignedPe $installerPath 0x014C

$distinctSigners = @($artifacts.SignerThumbprint | Sort-Object -Unique)
if ($distinctSigners.Count -ne 1) {
    throw "Release artifacts were signed by different certificates"
}
Assert-SlimeSignerThumbprint `
    $distinctSigners[0] `
    $ExpectedSignerThumbprint `
    "release artifacts"

$productVersion = (Get-Item -LiteralPath $installerPath).VersionInfo.ProductVersion
if (-not $productVersion -or $productVersion.Trim() -ne $Version) {
    throw "Installer product version '$productVersion' does not match $Version"
}
$sourceRevision = (Get-Item -LiteralPath $installerPath).VersionInfo.Comments
if ($sourceRevision -ne "Source revision: $ExpectedSourceRevision") {
    throw "Installer source revision metadata does not match $ExpectedSourceRevision"
}

foreach ($artifact in $artifacts) {
    Assert-SlimePeSourceRevision $artifact.Path $ExpectedSourceRevision
    Assert-SlimePeProductVersion $artifact.Path $Version
    Get-FileHash -Algorithm SHA256 -LiteralPath $artifact.Path
}
Write-Output "Windows $nativeDirectory release signature gate passed"
Write-Output "Source revision: $ExpectedSourceRevision"
