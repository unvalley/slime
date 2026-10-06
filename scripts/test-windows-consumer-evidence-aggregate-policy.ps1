$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-consumer-evidence.ps1")

function Assert-Rejected([scriptblock]$Action, [string]$ExpectedMessage) {
    try {
        & $Action
    } catch {
        if ($_.Exception.Message -notlike "*$ExpectedMessage*") {
            throw "Policy failed for an unexpected reason: $($_.Exception.Message)"
        }
        return
    }
    throw "Policy unexpectedly accepted inconsistent consumer evidence"
}

function Write-Evidence([string]$Name, [object]$Binding, [switch]$Duplicate) {
    $json = ConvertTo-SlimeWindowsReleaseBindingJson $Binding
    $success = if ($Name.EndsWith("-lifecycle.log")) {
        "Windows clean install/update/downgrade-rejection/uninstall gate passed"
    } else {
        "Windows signed consumer input and accessibility gate passed"
    }
    $contents = "$success`nRelease binding JSON: $json`n"
    if ($Duplicate) {
        $contents += "Release binding JSON: $json`n"
    }
    [IO.File]::WriteAllText(
        (Join-Path $temporaryDirectory $Name),
        $contents,
        [Text.UTF8Encoding]::new($false)
    )
}

$temporaryDirectory = Join-Path `
    ([IO.Path]::GetTempPath()) `
    "slime-windows-evidence-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
try {
    $x64 = [PSCustomObject][ordered]@{
        Architecture = "x64"
        Version = "1.2.3"
        SourceRevision = "a" * 40
        SignerThumbprint = "B" * 40
        InstallerSha256 = "C" * 64
        InstalledManifestSha256 = "D" * 64
    }
    $arm64x = [PSCustomObject][ordered]@{
        Architecture = "arm64x"
        Version = "1.2.3"
        SourceRevision = "a" * 40
        SignerThumbprint = "B" * 40
        InstallerSha256 = "E" * 64
        InstalledManifestSha256 = "F" * 64
    }
    Write-Evidence "x64-lifecycle.log" $x64
    Write-Evidence "x64-interactive.log" $x64
    Write-Evidence "arm64x-lifecycle.log" $arm64x
    Write-Evidence "arm64x-interactive.log" $arm64x
    & (Join-Path $PSScriptRoot "verify-windows-consumer-evidence.ps1") `
        -EvidenceDirectory $temporaryDirectory | Out-Null

    $changedX64 = $x64.PSObject.Copy()
    $changedX64.InstallerSha256 = "0" * 64
    Write-Evidence "x64-interactive.log" $changedX64
    Assert-Rejected {
        & (Join-Path $PSScriptRoot "verify-windows-consumer-evidence.ps1") `
            -EvidenceDirectory $temporaryDirectory | Out-Null
    } "refer to different releases"

    Write-Evidence "x64-interactive.log" $x64
    Write-Evidence "arm64x-interactive.log" $arm64x -Duplicate
    Assert-Rejected {
        & (Join-Path $PSScriptRoot "verify-windows-consumer-evidence.ps1") `
            -EvidenceDirectory $temporaryDirectory | Out-Null
    } "exactly one release binding"

    Write-Evidence "arm64x-lifecycle.log" $x64
    Write-Evidence "arm64x-interactive.log" $x64
    Assert-Rejected {
        & (Join-Path $PSScriptRoot "verify-windows-consumer-evidence.ps1") `
            -EvidenceDirectory $temporaryDirectory | Out-Null
    } "architecture does not match its file"

    $changedArm64x = $arm64x.PSObject.Copy()
    $changedArm64x.SignerThumbprint = "1" * 40
    Write-Evidence "arm64x-lifecycle.log" $changedArm64x
    Write-Evidence "arm64x-interactive.log" $changedArm64x
    Assert-Rejected {
        & (Join-Path $PSScriptRoot "verify-windows-consumer-evidence.ps1") `
            -EvidenceDirectory $temporaryDirectory | Out-Null
    } "differ in SignerThumbprint"

    Write-Evidence "arm64x-lifecycle.log" $arm64x
    Write-Evidence "arm64x-interactive.log" $arm64x
    $json = ConvertTo-SlimeWindowsReleaseBindingJson $arm64x
    [IO.File]::WriteAllText(
        (Join-Path $temporaryDirectory "arm64x-interactive.log"),
        "Release binding JSON: $json`n",
        [Text.UTF8Encoding]::new($false)
    )
    Assert-Rejected {
        & (Join-Path $PSScriptRoot "verify-windows-consumer-evidence.ps1") `
            -EvidenceDirectory $temporaryDirectory | Out-Null
    } "exactly one gate success marker"
} finally {
    if (Test-Path -LiteralPath $temporaryDirectory) {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
    }
}

Write-Output "Windows consumer evidence aggregate policy test passed"
