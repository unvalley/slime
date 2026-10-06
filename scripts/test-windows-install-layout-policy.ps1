$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-install-layout.ps1")

function Assert-Rejected([scriptblock]$Action, [string]$ExpectedMessage) {
    try {
        & $Action
    } catch {
        if ($_.Exception.Message -notlike "*$ExpectedMessage*") {
            throw "Policy failed for an unexpected reason: $($_.Exception.Message)"
        }
        return
    }
    throw "Policy unexpectedly accepted an invalid fixture"
}

$x64Specification = @(Get-SlimeNativePayloadSpecification "x64")
if ($x64Specification.Count -ne 4 -or
    @($x64Specification | Where-Object { $_.Machine -ne [UInt16]0x8664 }).Count -ne 0) {
    throw "x64 native payload specification is inconsistent"
}
$arm64Specification = @(Get-SlimeNativePayloadSpecification "arm64x")
$arm64Forwarders = @($arm64Specification | Where-Object { $_.Machine -eq 0 })
$arm64Implementations = @($arm64Specification | Where-Object { $_.Machine -eq [UInt16]0xAA64 })
$x64Implementations = @($arm64Specification | Where-Object { $_.Machine -eq [UInt16]0x8664 })
if ($arm64Specification.Count -ne 8 -or
    $arm64Forwarders.Count -ne 2 -or
    $arm64Implementations.Count -ne 4 -or
    $x64Implementations.Count -ne 2) {
    throw "ARM64X native payload specification is inconsistent"
}
Assert-Rejected {
    Get-SlimeNativePayloadSpecification "unsupported"
} "Unsupported native installer directory"

$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) `
    "slime-install-layout-$([guid]::NewGuid())"
try {
    New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
    $x64ExpectedFiles = Get-SlimeExpectedInstalledFiles
    foreach ($relativePath in $x64ExpectedFiles) {
        $platformPath = $relativePath.Replace(
            "\",
            [System.IO.Path]::DirectorySeparatorChar
        )
        $path = Join-Path $temporaryDirectory $platformPath
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        [System.IO.File]::WriteAllText($path, "fixture")
    }
    Assert-SlimeInstalledLayout $temporaryDirectory
    $manifestBefore = Get-SlimeInstalledManifestSha256 $temporaryDirectory
    if ($manifestBefore -notmatch '^[0-9A-F]{64}$') {
        throw "Installed manifest digest is malformed"
    }
    $settingsFixture = Join-Path $temporaryDirectory "x64\SlimeSettings.exe"
    [System.IO.File]::AppendAllText($settingsFixture, "changed")
    $manifestAfter = Get-SlimeInstalledManifestSha256 $temporaryDirectory
    if ($manifestAfter -eq $manifestBefore) {
        throw "Installed manifest digest did not change with a payload file"
    }

    $extraFile = Join-Path (Join-Path $temporaryDirectory "x64") "stale.dll"
    [System.IO.File]::WriteAllText($extraFile, "stale")
    Assert-Rejected {
        Assert-SlimeInstalledLayout $temporaryDirectory
    } "file list is unexpected"
    Remove-Item -LiteralPath $extraFile

    $extraDirectory = Join-Path $temporaryDirectory "old-version"
    New-Item -ItemType Directory -Path $extraDirectory | Out-Null
    Assert-Rejected {
        Assert-SlimeInstalledLayout $temporaryDirectory
    } "directory list is unexpected"
    Remove-Item -LiteralPath $extraDirectory

    $uninstaller = Join-Path $temporaryDirectory "Uninstall.exe"
    $properties = [PSCustomObject]@{
        DisplayName = "Slime Japanese IME"
        DisplayVersion = "0.1.0"
        SourceRevision = "development"
        Publisher = "unvalley"
        DisplayIcon = Join-Path $temporaryDirectory "x64\SlimeSettings.exe"
        UninstallString = "`"$uninstaller`""
        QuietUninstallString = "`"$uninstaller`" /S"
        NoModify = 1
        NoRepair = 1
    }
    Assert-SlimeUninstallRegistration $properties "0.1.0" $temporaryDirectory
    $properties.Publisher = "unexpected"
    Assert-Rejected {
        Assert-SlimeUninstallRegistration $properties "0.1.0" $temporaryDirectory
    } "do not match"
    $properties.Publisher = "unvalley"
    $properties.SourceRevision = "unexpected"
    Assert-Rejected {
        Assert-SlimeUninstallRegistration $properties "0.1.0" $temporaryDirectory
    } "do not match"

    $programFilesFixture = Join-Path $temporaryDirectory "program-files"
    New-Item -ItemType Directory -Path $programFilesFixture | Out-Null
    Assert-SlimeInstallDirectoriesAbsent $programFilesFixture
    New-Item -ItemType Directory -Path (Join-Path $programFilesFixture "Slime") | Out-Null
    Assert-Rejected {
        Assert-SlimeInstallDirectoriesAbsent $programFilesFixture
    } "remains without an active release"

    $arm64Layout = Join-Path $temporaryDirectory "arm64-layout"
    New-Item -ItemType Directory -Path $arm64Layout | Out-Null
    $arm64ExpectedFiles = Get-SlimeExpectedInstalledFiles "arm64x"
    foreach ($relativePath in $arm64ExpectedFiles) {
        $platformPath = $relativePath.Replace(
            "\",
            [System.IO.Path]::DirectorySeparatorChar
        )
        $path = Join-Path $arm64Layout $platformPath
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        [System.IO.File]::WriteAllText($path, "fixture")
    }
    Assert-SlimeInstalledLayout $arm64Layout "arm64x"
    if ((Get-SlimeInstalledManifestSha256 $arm64Layout "arm64x") -notmatch
        '^[0-9A-F]{64}$') {
        throw "ARM64X installed manifest digest is malformed"
    }
    $arm64Uninstaller = Join-Path $arm64Layout "Uninstall.exe"
    $arm64Properties = [PSCustomObject]@{
        DisplayName = "Slime Japanese IME"
        DisplayVersion = "0.1.0"
        SourceRevision = "development"
        Publisher = "unvalley"
        DisplayIcon = Join-Path $arm64Layout "arm64x\SlimeSettings.exe"
        UninstallString = "`"$arm64Uninstaller`""
        QuietUninstallString = "`"$arm64Uninstaller`" /S"
        NoModify = 1
        NoRepair = 1
    }
    Assert-SlimeUninstallRegistration $arm64Properties "0.1.0" $arm64Layout "arm64x"
} finally {
    if (Test-Path -LiteralPath $temporaryDirectory) {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
    }
}

Write-Output "Windows installed-layout policy test passed"
