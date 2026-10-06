param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [string]$PayloadX64,

    [string]$PayloadARM64X,

    [Parameter(Mandatory = $true)]
    [string]$PayloadX86,

    [Parameter(Mandatory = $true)]
    [string]$CurrentInstaller,

    [Parameter(Mandatory = $true)]
    [string]$PreviousInstaller,

    [Parameter(Mandatory = $true)]
    [string]$FailingInstaller
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-install-layout.ps1")
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

if ($env:SLIME_DEVELOPMENT_TEST_VM -ne "1") {
    throw "Run only in a disposable Windows VM with SLIME_DEVELOPMENT_TEST_VM=1"
}
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "The development lifecycle test requires an elevated PowerShell session"
}
if ((Test-Path $productKey) -or (Test-Path $com64) -or (Test-Path $com32) -or (Test-Path $profile)) {
    throw "The disposable VM is not clean"
}
Assert-SlimeInstallDirectoriesAbsent $env:ProgramFiles
Assert-SlimeStartMenuRemoved

function Assert-UnsignedPe([string]$Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::NotSigned) {
        throw "Development lifecycle artifacts must be unsigned: $Path ($($signature.Status))"
    }
}

function Install-Package([string]$Path) {
    Assert-UnsignedPe $Path
    $process = Start-Process -FilePath $Path -ArgumentList "/S" -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "Installer failed with exit code $($process.ExitCode): $Path"
    }
}

function Assert-PackageRejected([string]$Path) {
    Assert-UnsignedPe $Path
    $process = Start-Process -FilePath $Path -ArgumentList "/S" -Wait -PassThru
    if ($process.ExitCode -eq 0) {
        throw "Intentionally invalid installer unexpectedly succeeded: $Path"
    }
}

function Get-PayloadHashes([string]$Native, [string]$X86) {
    $hashes = @{}
    foreach ($architecture in @(
        @{ Name = $nativeDirectory; Path = $Native; Files = $nativeFiles },
        @{ Name = "x86"; Path = $X86; Files = $requiredFiles }
    )) {
        foreach ($file in $architecture.Files) {
            $path = Join-Path $architecture.Path $file
            Assert-UnsignedPe $path
            $hashes["$($architecture.Name)\$file"] = (
                Get-FileHash -Algorithm SHA256 -LiteralPath $path
            ).Hash
        }
    }
    return $hashes
}

function Assert-Installed([string]$ExpectedVersion, [hashtable]$ExpectedHashes) {
    if (-not (Test-Path $productKey)) {
        throw "Uninstall registration is missing"
    }
    $properties = Get-ItemProperty $productKey
    if ($properties.DisplayVersion -ne $ExpectedVersion) {
        throw "Installed version '$($properties.DisplayVersion)' does not match $ExpectedVersion"
    }
    $expectedDirectory = Join-Path $env:ProgramFiles "Slime\$ExpectedVersion"
    $installDirectory = [System.IO.Path]::GetFullPath($properties.InstallLocation)
    if ($installDirectory -ne [System.IO.Path]::GetFullPath($expectedDirectory)) {
        throw "InstallLocation is outside the expected versioned directory: $installDirectory"
    }
    Assert-SlimeInstalledLayout $installDirectory $nativeDirectory
    Assert-SlimeUninstallRegistration `
        $properties $ExpectedVersion $installDirectory $nativeDirectory "development"
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
            $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
            if ($actualHash -ne $ExpectedHashes[$key]) {
                throw "Installed payload differs from the build input: $key"
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
    if (-not (Test-Path $profile)) {
        throw "Japanese language profile is missing"
    }
    $uninstaller = Join-Path $installDirectory "Uninstall.exe"
    Assert-UnsignedPe $uninstaller
    if ($properties.UninstallString -notlike "*$uninstaller*") {
        throw "Uninstall registration does not point to the installed uninstaller"
    }
    return [PSCustomObject]@{
        Directory = $installDirectory
        Uninstaller = $uninstaller
    }
}

$nativePayloadPath = if ($hasARM64X) {
    (Resolve-Path $PayloadARM64X).Path
} else {
    (Resolve-Path $PayloadX64).Path
}
$payloadX86Path = (Resolve-Path $PayloadX86).Path
$currentInstallerPath = (Resolve-Path $CurrentInstaller).Path
$previousInstallerPath = (Resolve-Path $PreviousInstaller).Path
$failingInstallerPath = (Resolve-Path $FailingInstaller).Path
$expectedHashes = Get-PayloadHashes $nativePayloadPath $payloadX86Path

$previousVersion = (Get-Item -LiteralPath $previousInstallerPath).VersionInfo.ProductVersion.Trim()
if ($previousVersion -notmatch '^\d+\.\d+\.\d+$' -or [version]$previousVersion -ge [version]$Version) {
    throw "Previous installer $previousVersion must be older than $Version"
}
$failingVersion = (Get-Item -LiteralPath $failingInstallerPath).VersionInfo.ProductVersion.Trim()
if ($failingVersion -notmatch '^\d+\.\d+\.\d+$' -or
    [version]$failingVersion -le [version]$previousVersion -or
    [version]$failingVersion -ge [version]$Version) {
    throw "Failing installer $failingVersion must be between $previousVersion and $Version"
}

$userDataDirectory = Join-Path $env:LOCALAPPDATA "Slime"
$sentinel = Join-Path $userDataDirectory "development-lifecycle-sentinel.txt"
if (Test-Path -LiteralPath $sentinel) {
    throw "The disposable VM is not clean: user-data sentinel already exists"
}
New-Item -ItemType Directory -Path $userDataDirectory -Force | Out-Null
[System.IO.File]::WriteAllText($sentinel, "preserve user data")

Install-Package $previousInstallerPath
$previousInstall = Assert-Installed $previousVersion $expectedHashes
if ((Get-Content -LiteralPath $sentinel -Raw) -ne "preserve user data") {
    throw "Clean install changed existing user data"
}

Assert-PackageRejected $failingInstallerPath
$restoredInstall = Assert-Installed $previousVersion $expectedHashes
if ($restoredInstall.Directory -ne $previousInstall.Directory) {
    throw "Failed update did not restore the previous install directory"
}
$failedDirectory = Join-Path $env:ProgramFiles "Slime\$failingVersion"
if (Test-Path -LiteralPath $failedDirectory) {
    throw "Failed update left a partial install directory: $failedDirectory"
}
if ((Get-Content -LiteralPath $sentinel -Raw) -ne "preserve user data") {
    throw "Failed update changed existing user data"
}

Install-Package $currentInstallerPath
$currentInstall = Assert-Installed $Version $expectedHashes
if ($previousInstall.Directory -eq $currentInstall.Directory) {
    throw "Update did not move to a versioned install directory"
}
if (Test-Path -LiteralPath $previousInstall.Directory) {
    throw "Previous version directory remains after update: $($previousInstall.Directory)"
}
if ((Get-Content -LiteralPath $sentinel -Raw) -ne "preserve user data") {
    throw "Update changed existing user data"
}

Assert-PackageRejected $previousInstallerPath
$afterDowngradeAttempt = Assert-Installed $Version $expectedHashes
if ($afterDowngradeAttempt.Directory -ne $currentInstall.Directory) {
    throw "Rejected downgrade changed the current install directory"
}
if (Test-Path -LiteralPath $previousInstall.Directory) {
    throw "Rejected downgrade restored the previous version directory"
}
if ((Get-Content -LiteralPath $sentinel -Raw) -ne "preserve user data") {
    throw "Rejected downgrade changed existing user data"
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
if ((Get-Content -LiteralPath $sentinel -Raw) -ne "preserve user data") {
    throw "Uninstall removed or changed user data"
}
Remove-Item -LiteralPath $sentinel

Write-Output "Unsigned Windows clean install/update/downgrade-rejection/uninstall smoke test passed"
Write-Output "This is development evidence only; it does not satisfy the signed release gate."
