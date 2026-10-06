param(
    [Parameter(Mandatory = $true)]
    [string]$EvidenceDirectory
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-consumer-evidence.ps1")

$directory = (Resolve-Path -LiteralPath $EvidenceDirectory).Path
$directoryItem = Get-Item -LiteralPath $directory -Force
if (-not $directoryItem.PSIsContainer -or
    ($directoryItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Consumer evidence directory must not be a reparse point"
}
$requiredEvidence = [ordered]@{
    "x64-lifecycle.log" = @{
        Architecture = "x64"
        Success = "Windows clean install/update/downgrade-rejection/uninstall gate passed"
    }
    "x64-interactive.log" = @{
        Architecture = "x64"
        Success = "Windows signed consumer input and accessibility gate passed"
    }
    "arm64x-lifecycle.log" = @{
        Architecture = "arm64x"
        Success = "Windows clean install/update/downgrade-rejection/uninstall gate passed"
    }
    "arm64x-interactive.log" = @{
        Architecture = "arm64x"
        Success = "Windows signed consumer input and accessibility gate passed"
    }
}
$bindings = @{}

foreach ($entry in $requiredEvidence.GetEnumerator()) {
    $path = Join-Path $directory $entry.Key
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Consumer evidence is missing: $($entry.Key)"
    }
    $item = Get-Item -LiteralPath $path -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Consumer evidence must not be a reparse point: $($entry.Key)"
    }
    if ($item.Length -gt 1048576) {
        throw "Consumer evidence exceeds the size limit: $($entry.Key)"
    }
    $lines = @(Get-Content -LiteralPath $path)
    $successLines = @($lines | Where-Object { $_ -ceq $entry.Value.Success })
    if ($successLines.Count -ne 1) {
        throw "Consumer evidence must contain exactly one gate success marker: $($entry.Key)"
    }
    $releaseBindingLines = @(
        $lines | Where-Object { $_.StartsWith("Release binding JSON: ") }
    )
    if ($releaseBindingLines.Count -ne 1) {
        throw "Consumer evidence must contain exactly one release binding: $($entry.Key)"
    }
    $json = $releaseBindingLines[0].Substring("Release binding JSON: ".Length)
    try {
        $binding = $json | ConvertFrom-Json
    } catch {
        throw "Consumer evidence contains malformed release JSON: $($entry.Key)"
    }
    Assert-SlimeWindowsConsumerBinding $binding
    if ($binding.Architecture -cne $entry.Value.Architecture) {
        throw "Consumer evidence architecture does not match its file: $($entry.Key)"
    }
    $canonicalJson = ConvertTo-SlimeWindowsReleaseBindingJson $binding
    if ($json -cne $canonicalJson) {
        throw "Consumer evidence release binding is not canonical: $($entry.Key)"
    }
    $bindings[$entry.Key] = [PSCustomObject]@{
        Binding = $binding
        Json = $canonicalJson
    }
}

foreach ($architecture in @("x64", "arm64x")) {
    $lifecycle = $bindings["$architecture-lifecycle.log"]
    $interactive = $bindings["$architecture-interactive.log"]
    if ($lifecycle.Json -cne $interactive.Json) {
        throw "$architecture lifecycle and interactive evidence refer to different releases"
    }
}

$x64Binding = $bindings["x64-lifecycle.log"].Binding
$arm64xBinding = $bindings["arm64x-lifecycle.log"].Binding
foreach ($name in @("Version", "SourceRevision", "SignerThumbprint")) {
    if ($x64Binding.$name -cne $arm64xBinding.$name) {
        throw "x64 and arm64x consumer evidence differ in $name"
    }
}

Write-Output "Windows x64 and arm64x consumer evidence gate passed"
Write-Output "x64 release binding JSON: $($bindings['x64-lifecycle.log'].Json)"
Write-Output "arm64x release binding JSON: $($bindings['arm64x-lifecycle.log'].Json)"
