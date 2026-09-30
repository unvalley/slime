param(
    [Parameter(Mandatory = $true)]
    [string]$Artifact
)

$ErrorActionPreference = "Stop"
$artifactPath = (Resolve-Path $Artifact).Path
$verifier = Join-Path $PSScriptRoot "verify-windows-arm64x-artifact.ps1"
$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) `
    "slime-arm64x-policy-$([guid]::NewGuid())"
$fixture = Join-Path $temporaryDirectory "artifact"

function Reset-Fixture {
    if (Test-Path -LiteralPath $fixture) {
        Remove-Item -LiteralPath $fixture -Recurse -Force
    }
    Copy-Item -LiteralPath $artifactPath -Destination $fixture -Recurse
}

function Assert-Rejected([scriptblock]$Action, [string]$ExpectedMessage) {
    try {
        & $Action
    } catch {
        if ($_.Exception.Message -notlike "*$ExpectedMessage*") {
            throw "ARM64X policy failed for an unexpected reason: $($_.Exception.Message)"
        }
        return
    }
    throw "ARM64X policy unexpectedly accepted an invalid artifact"
}

try {
    New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
    $comments = (Get-Item -LiteralPath (Join-Path $artifactPath "SlimeIME.dll")).VersionInfo.Comments
    if ($comments -notmatch '^Source revision: ([0-9a-f]{40})$') {
        throw "ARM64X fixture lacks a valid source revision"
    }
    $sourceRevision = $Matches[1]
    $productVersion = (Get-Item -LiteralPath (Join-Path $artifactPath "SlimeIME.dll")).VersionInfo.ProductVersion
    if (-not $productVersion -or $productVersion.Trim() -notmatch '^\d+\.\d+\.\d+$') {
        throw "ARM64X fixture lacks a valid product version"
    }
    $productVersion = $productVersion.Trim()
    & $verifier `
        -Artifact $artifactPath `
        -ExpectedSourceRevision $sourceRevision `
        -ExpectedVersion $productVersion | Out-Null

    Reset-Fixture
    [System.IO.File]::WriteAllText((Join-Path $fixture "unexpected.txt"), "fixture")
    Assert-Rejected {
        & $verifier -Artifact $fixture | Out-Null
    } "file list is unexpected"

    Reset-Fixture
    New-Item -ItemType Directory -Path (Join-Path $fixture "nested") | Out-Null
    Assert-Rejected {
        & $verifier -Artifact $fixture | Out-Null
    } "flat directory"

    Reset-Fixture
    Remove-Item -LiteralPath (Join-Path $fixture "slime_ffi_arm64.dll")
    Assert-Rejected {
        & $verifier -Artifact $fixture | Out-Null
    } "file list is unexpected"

    Reset-Fixture
    Copy-Item `
        -LiteralPath (Join-Path $fixture "SlimeIME_arm64.dll") `
        -Destination (Join-Path $fixture "SlimeIME_x64.dll") `
        -Force
    Assert-Rejected {
        & $verifier -Artifact $fixture | Out-Null
    } "Unexpected PE machine"
} finally {
    if (Test-Path -LiteralPath $temporaryDirectory) {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
    }
}

Write-Output "Windows ARM64X artifact policy test passed"
