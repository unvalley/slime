$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-release-source.ps1")

$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) `
    "slime-release-source-$([guid]::NewGuid())"
try {
    New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
    & git -C $temporaryDirectory init --quiet
    if ($LASTEXITCODE -ne 0) { throw "Could not create release source fixture" }
    & git -C $temporaryDirectory config user.name "Release Policy Fixture"
    & git -C $temporaryDirectory config user.email "release-policy@example.invalid"
    [System.IO.File]::WriteAllText(
        (Join-Path $temporaryDirectory "source.txt"),
        "committed`n"
    )
    & git -C $temporaryDirectory add source.txt
    & git -C $temporaryDirectory commit --quiet -m "test: add release source fixture"
    if ($LASTEXITCODE -ne 0) { throw "Could not commit release source fixture" }

    $expectedRevision = (& git -C $temporaryDirectory rev-parse HEAD).Trim()
    $actualRevision = Get-SlimeCleanReleaseSourceRevision $temporaryDirectory
    if ($actualRevision -ne $expectedRevision) {
        throw "Clean Windows release source returned the wrong revision"
    }

    [System.IO.File]::AppendAllText(
        (Join-Path $temporaryDirectory "source.txt"),
        "changed`n"
    )
    try {
        $null = Get-SlimeCleanReleaseSourceRevision $temporaryDirectory
        throw "Windows release source policy accepted a tracked modification"
    } catch {
        if ($_.Exception.Message -notlike "*clean source checkout*") { throw }
    }
    & git -C $temporaryDirectory restore source.txt

    [System.IO.File]::WriteAllText(
        (Join-Path $temporaryDirectory "untracked.txt"),
        "untracked`n"
    )
    try {
        $null = Get-SlimeCleanReleaseSourceRevision $temporaryDirectory
        throw "Windows release source policy accepted an untracked file"
    } catch {
        if ($_.Exception.Message -notlike "*clean source checkout*") { throw }
    }
    Remove-Item -LiteralPath (Join-Path $temporaryDirectory "untracked.txt")

    $nestedDirectory = Join-Path $temporaryDirectory "nested"
    New-Item -ItemType Directory -Path $nestedDirectory | Out-Null
    try {
        $null = Get-SlimeCleanReleaseSourceRevision $nestedDirectory
        throw "Windows release source policy accepted a nested build directory"
    } catch {
        if ($_.Exception.Message -notlike "*repository root*") { throw }
    }
} finally {
    if (Test-Path -LiteralPath $temporaryDirectory) {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
    }
}

Write-Output "Windows release source policy test passed"
