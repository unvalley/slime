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
    [string]$ExpectedSourceRevision,

    [string]$PreviousInstaller,

    [ValidatePattern('^[0-9A-Fa-f]{40}$')]
    [string]$PreviousExpectedSignerThumbprint
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-signature.ps1")
. (Join-Path $PSScriptRoot "windows-install-layout.ps1")
. (Join-Path $PSScriptRoot "windows-consumer-evidence.ps1")
$productKey = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\SlimeIME"
$textServiceClsid = "{E4F851DD-9801-4582-B84F-7D76B7EEC049}"
$profileGuid = "{C2B62953-18E5-4DFB-93AD-D407017A9E99}"
$com64 = "HKLM:\Software\Classes\CLSID\$textServiceClsid\InprocServer32"
$com32 = "HKLM:\Software\Classes\WOW6432Node\CLSID\$textServiceClsid\InprocServer32"
$profile = "HKLM:\Software\Microsoft\CTF\TIP\$textServiceClsid\LanguageProfile\0x00000411\$profileGuid"
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
$nativeFiles = Get-SlimeNativePayloadFiles $nativeDirectory

if ($env:SLIME_RELEASE_TEST_VM -ne "1") {
    throw "Run only in a disposable Windows VM with SLIME_RELEASE_TEST_VM=1"
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "The lifecycle gate requires an elevated PowerShell session"
}
if (Test-Path $productKey) {
    throw "The VM is not clean: Slime is already registered for uninstall"
}
if ((Test-Path $com64) -or (Test-Path $com32) -or (Test-Path $profile)) {
    throw "The VM is not clean: an input service registration already exists"
}
Assert-SlimeInstallDirectoriesAbsent $env:ProgramFiles
Assert-SlimeStartMenuRemoved

function Assert-ValidSignature([string]$Path) {
    return Assert-SlimeAuthenticodeSignature $Path
}

function Install-Package([string]$Path) {
    $resolved = (Resolve-Path $Path).Path
    $null = Assert-ValidSignature $resolved
    $process = Start-Process -FilePath $resolved -ArgumentList "/S" -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "Installer failed with exit code $($process.ExitCode): $resolved"
    }
}

function Assert-PackageRejected([string]$Path) {
    $resolved = (Resolve-Path $Path).Path
    $null = Assert-ValidSignature $resolved
    $process = Start-Process -FilePath $resolved -ArgumentList "/S" -Wait -PassThru
    if ($process.ExitCode -eq 0) {
        throw "Older signed installer unexpectedly succeeded: $resolved"
    }
}

function Assert-Installed(
    [string]$ExpectedVersion,
    [string]$ExpectedSigner,
    [hashtable]$ExpectedHashes,
    [string]$ExpectedRevision
) {
    if (-not (Test-Path $productKey)) { throw "Uninstall registration is missing" }
    $properties = Get-ItemProperty $productKey
    $installDirectory = $properties.InstallLocation
    if (-not $installDirectory -or -not (Test-Path -LiteralPath $installDirectory)) {
        throw "InstallLocation is missing or invalid"
    }
    if ($properties.DisplayVersion -ne $ExpectedVersion) {
        throw "Installed version '$($properties.DisplayVersion)' does not match $ExpectedVersion"
    }
    $expectedDirectory = Join-Path $env:ProgramFiles "Slime\$ExpectedVersion"
    if ([System.IO.Path]::GetFullPath($installDirectory) -ne [System.IO.Path]::GetFullPath($expectedDirectory)) {
        throw "InstallLocation is outside the expected versioned directory: $installDirectory"
    }
    Assert-SlimeInstalledLayout $installDirectory $nativeDirectory
    Assert-SlimeUninstallRegistration `
        $properties $ExpectedVersion $installDirectory $nativeDirectory $ExpectedRevision
    Assert-SlimeStartMenu $installDirectory $nativeDirectory
    foreach ($architecture in @(
        @{ Name = $nativeDirectory; Files = $nativeFiles },
        @{ Name = "x86"; Files = $requiredFiles }
    )) {
        foreach ($file in $architecture.Files) {
            $key = "$($architecture.Name)\$file"
            $path = Join-Path $installDirectory $key
            if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
                throw "Installed payload is missing: $path"
            }
            $signature = Assert-ValidSignature $path
            if ($signature.SignerCertificate.Thumbprint -ne $ExpectedSigner) {
                throw "Installed payload signer differs from the installer: $path"
            }
            if ($ExpectedHashes) {
                $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
                if ($actualHash -ne $ExpectedHashes[$key]) {
                    throw "Installed payload differs from the signed release input: $key"
                }
            }
        }
    }
    Assert-SlimeSettingsSelfTest (
        Join-Path $installDirectory "$nativeDirectory\SlimeSettings.exe"
    )
    Assert-SlimeSettingsSelfTest (Join-Path $installDirectory "x86\SlimeSettings.exe")
    if (-not (Test-Path $com64) -or -not (Test-Path $com32)) {
        throw "Both native 64-bit and x86 COM registrations are required"
    }
    $registered64 = (Get-Item $com64).GetValue("")
    $registered32 = (Get-Item $com32).GetValue("")
    if ($registered64 -ne (Join-Path $installDirectory "$nativeDirectory\SlimeIME.dll")) {
        throw "native COM registration points to an unexpected path: $registered64"
    }
    if ($registered32 -ne (Join-Path $installDirectory "x86\SlimeIME.dll")) {
        throw "x86 COM registration points to an unexpected path: $registered32"
    }
    if (-not (Test-Path $profile)) { throw "Japanese language profile is missing" }
    $uninstaller = Join-Path $installDirectory "Uninstall.exe"
    $uninstallerSignature = Assert-ValidSignature $uninstaller
    if ($uninstallerSignature.SignerCertificate.Thumbprint -ne $ExpectedSigner) {
        throw "Embedded uninstaller signer differs from the outer installer"
    }
    if ($properties.UninstallString -notlike "*$uninstaller*") {
        throw "Uninstall registration does not point to the signed uninstaller"
    }
    return [PSCustomObject]@{
        Directory = $installDirectory
        Version = $properties.DisplayVersion
        Uninstaller = $uninstaller
    }
}

$nativePayloadPath = if ($hasARM64X) {
    (Resolve-Path $PayloadARM64X).Path
} else {
    (Resolve-Path $PayloadX64).Path
}
$payloadX86Path = (Resolve-Path $PayloadX86).Path
$installerPath = (Resolve-Path $Installer).Path
$installerHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $installerPath).Hash
$releaseVerification = @{
    Version = $Version
    PayloadX86 = $payloadX86Path
    Installer = $installerPath
    ExpectedSignerThumbprint = $ExpectedSignerThumbprint
    ExpectedSourceRevision = $ExpectedSourceRevision
}
if ($hasARM64X) {
    $releaseVerification.PayloadARM64X = $nativePayloadPath
} else {
    $releaseVerification.PayloadX64 = $nativePayloadPath
}
& "$PSScriptRoot\verify-windows-release.ps1" @releaseVerification

$installerSignature = Assert-ValidSignature $installerPath
$currentSigner = $installerSignature.SignerCertificate.Thumbprint
Assert-SlimeSignerThumbprint $currentSigner $ExpectedSignerThumbprint $installerPath
$expectedHashes = @{}
foreach ($architecture in @(
    @{ Name = $nativeDirectory; Path = $nativePayloadPath; Files = $nativeFiles },
    @{ Name = "x86"; Path = $payloadX86Path; Files = $requiredFiles }
)) {
    foreach ($file in $architecture.Files) {
        $key = "$($architecture.Name)\$file"
        $expectedHashes[$key] = (
            Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $architecture.Path $file)
        ).Hash
    }
}

$userDataDirectory = Join-Path $env:LOCALAPPDATA "Slime"
$sentinel = Join-Path $userDataDirectory "release-gate-sentinel.txt"
if (Test-Path -LiteralPath $sentinel) {
    throw "The VM is not clean: user-data sentinel already exists"
}
New-Item -ItemType Directory -Path $userDataDirectory -Force | Out-Null
[System.IO.File]::WriteAllText($sentinel, "preserve user data")

$previousInstall = $null
if ($PreviousInstaller) {
    $previousPath = (Resolve-Path $PreviousInstaller).Path
    $previousSignature = Assert-ValidSignature $previousPath
    $previousSigner = $previousSignature.SignerCertificate.Thumbprint
    $approvedPreviousSigner = if ($PreviousExpectedSignerThumbprint) {
        $PreviousExpectedSignerThumbprint
    } else {
        $ExpectedSignerThumbprint
    }
    Assert-SlimeSignerThumbprint `
        $previousSigner $approvedPreviousSigner $previousPath
    $previousVersion = (Get-Item -LiteralPath $previousPath).VersionInfo.ProductVersion.Trim()
    if ($previousVersion -notmatch '^\d+\.\d+\.\d+$') {
        throw "Previous installer version is invalid: $previousVersion"
    }
    if ([version]$previousVersion -ge [version]$Version) {
        throw "Previous installer $previousVersion must be older than $Version"
    }
    Install-Package $previousPath
    $previousSourceRevision = (Get-Item -LiteralPath $previousPath).VersionInfo.Comments
    if ($previousSourceRevision -notmatch '^Source revision: ([0-9a-f]{40})$') {
        throw "Previous installer source revision metadata is invalid"
    }
    $previousInstall = Assert-Installed `
        $previousVersion $previousSigner $null $Matches[1]
    if ((Get-Content -LiteralPath $sentinel -Raw) -ne "preserve user data") {
        throw "Clean install changed existing user data"
    }
}

Install-Package $installerPath
$currentInstall = Assert-Installed `
    $Version $currentSigner $expectedHashes $ExpectedSourceRevision
$installedManifestHash = Get-SlimeInstalledManifestSha256 `
    $currentInstall.Directory $nativeDirectory
if ($previousInstall -and $previousInstall.Directory -eq $currentInstall.Directory) {
    throw "Update did not move to a versioned install directory"
}
if ($previousInstall -and (Test-Path -LiteralPath $previousInstall.Directory)) {
    throw "Previous version directory remains after update: $($previousInstall.Directory)"
}
if ((Get-Content -LiteralPath $sentinel -Raw) -ne "preserve user data") {
    throw "Update changed existing user data"
}

Install-Package $installerPath
$afterSameVersionInstall = Assert-Installed `
    $Version $currentSigner $expectedHashes $ExpectedSourceRevision
$afterSameVersionManifest = Get-SlimeInstalledManifestSha256 `
    $afterSameVersionInstall.Directory $nativeDirectory
if ($afterSameVersionInstall.Directory -ne $currentInstall.Directory -or
    $afterSameVersionManifest -ne $installedManifestHash) {
    throw "Same-version reinstall changed the installed release"
}
if ((Get-Content -LiteralPath $sentinel -Raw) -ne "preserve user data") {
    throw "Same-version reinstall changed existing user data"
}

if ($PreviousInstaller) {
    Assert-PackageRejected $previousPath
    $afterDowngradeAttempt = Assert-Installed `
        $Version $currentSigner $expectedHashes $ExpectedSourceRevision
    $afterDowngradeManifest = Get-SlimeInstalledManifestSha256 `
        $afterDowngradeAttempt.Directory $nativeDirectory
    if ($afterDowngradeManifest -ne $installedManifestHash) {
        throw "Rejected downgrade changed the installed manifest"
    }
    if ($afterDowngradeAttempt.Directory -ne $currentInstall.Directory) {
        throw "Rejected downgrade changed the current install directory"
    }
    if (Test-Path -LiteralPath $previousInstall.Directory) {
        throw "Rejected downgrade restored the previous version directory"
    }
    if ((Get-Content -LiteralPath $sentinel -Raw) -ne "preserve user data") {
        throw "Rejected downgrade changed existing user data"
    }
}

$process = Start-Process -FilePath $currentInstall.Uninstaller -ArgumentList "/S" -Wait -PassThru
if ($process.ExitCode -ne 0) {
    throw "Uninstaller failed with exit code $($process.ExitCode)"
}
if ((Test-Path $productKey) -or (Test-Path $com64) -or (Test-Path $com32) -or (Test-Path $profile)) {
    throw "Registration remains after uninstall"
}
Assert-SlimeStartMenuRemoved
Assert-SlimeInstallDirectoriesAbsent $env:ProgramFiles
if (Test-Path -LiteralPath $currentInstall.Directory) {
    throw "Install directory remains after uninstall: $($currentInstall.Directory)"
}
if (-not (Test-Path -LiteralPath $sentinel)) {
    throw "Uninstall removed user data"
}
Remove-Item -LiteralPath $sentinel

Write-Output "Windows clean install/update/downgrade-rejection/uninstall gate passed"
$releaseBinding = [PSCustomObject][ordered]@{
    Architecture = $nativeDirectory
    Version = $Version
    SourceRevision = $ExpectedSourceRevision
    SignerThumbprint = $currentSigner.ToUpperInvariant()
    InstallerSha256 = $installerHash
    InstalledManifestSha256 = $installedManifestHash
}
Write-Output "Architecture: $nativeDirectory"
Write-Output "Version: $Version"
Write-Output "Signer thumbprint: $($releaseBinding.SignerThumbprint)"
Write-Output "Source revision: $ExpectedSourceRevision"
Write-Output "Release installer SHA-256: $installerHash"
Write-Output "Installed manifest SHA-256: $installedManifestHash"
$releaseBindingJson = ConvertTo-SlimeWindowsReleaseBindingJson $releaseBinding
Write-Output "Release binding JSON: $releaseBindingJson"
