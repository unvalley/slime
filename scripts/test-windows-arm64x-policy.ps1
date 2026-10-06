$ErrorActionPreference = "Stop"
$repository = Split-Path -Parent $PSScriptRoot
$manifestDirectory = Join-Path $repository "platforms/windows/arm64x"

function Get-ManifestExports([string]$Name) {
    $path = Join-Path $manifestDirectory $Name
    $exports = @(
        Get-Content -LiteralPath $path |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_ -ne "" } |
            Sort-Object
    )
    if ($exports.Count -eq 0 -or
        @($exports | Where-Object { $_ -notmatch '^[A-Za-z_][A-Za-z0-9_]*$' }).Count -ne 0 -or
        @($exports | Sort-Object -Unique).Count -ne $exports.Count) {
        throw "Invalid ARM64X export manifest: $path"
    }
    return $exports
}

function Assert-ExportSet(
    [string[]]$Expected,
    [string[]]$Actual,
    [string]$Description
) {
    $difference = @(Compare-Object $Expected $Actual)
    if ($difference.Count -ne 0) {
        throw "$Description export manifest is stale: $($difference | Out-String)"
    }
}

$ffiHeaderPath = Join-Path $repository "crates/slime-ffi/include/slime_ffi.h"
$ffiHeader = Get-Content -LiteralPath $ffiHeaderPath -Raw
$ffiExports = @(
    [regex]::Matches($ffiHeader, '\b(slime_[a-z0-9_]+)\s*\(') |
        ForEach-Object { $_.Groups[1].Value } |
        Sort-Object -Unique
)
Assert-ExportSet `
    (Get-ManifestExports "slime-ffi-exports.txt") `
    $ffiExports `
    "Rust C ABI"

$imeDefinitionPath = Join-Path $repository "platforms/windows/native/src/SlimeIME.def"
$imeExports = @(
    Get-Content -LiteralPath $imeDefinitionPath |
        ForEach-Object { $_.Trim() } |
        Where-Object { $_ -ne "" -and $_ -notmatch '^(LIBRARY|EXPORTS)(\s|$)' } |
        ForEach-Object { ($_ -split '\s+')[0] } |
        Sort-Object -Unique
)
Assert-ExportSet `
    (Get-ManifestExports "slime-ime-exports.txt") `
    $imeExports `
    "TSF COM"

$registerHelperPath = Join-Path $repository "platforms/windows/native/src/RegisterIME.cpp"
$registerHelper = Get-Content -LiteralPath $registerHelperPath -Raw
$probeBlock = [regex]::Match(
    $registerHelper,
    'requiredExports\[\]\s*=\s*\{(?<body>.*?)\};',
    [System.Text.RegularExpressions.RegexOptions]::Singleline
)
if (-not $probeBlock.Success) {
    throw "ARM64X load-probe export list was not found: $registerHelperPath"
}
$probeExports = @(
    [regex]::Matches($probeBlock.Groups["body"].Value, '"([A-Za-z_][A-Za-z0-9_]*)"') |
        ForEach-Object { $_.Groups[1].Value } |
        Sort-Object -Unique
)
Assert-ExportSet `
    (Get-ManifestExports "slime-ime-exports.txt") `
    $probeExports `
    "TSF load probe"

Write-Output "Windows ARM64X export policy test passed"
