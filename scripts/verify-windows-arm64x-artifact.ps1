param(
    [Parameter(Mandatory = $true)]
    [string]$Artifact,

    [string]$Dumpbin = "dumpbin.exe",

    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$ExpectedSourceRevision,

    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$ExpectedVersion,

    [switch]$AllowSigned
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-signature.ps1")
$repository = Split-Path -Parent $PSScriptRoot
$artifactPath = (Resolve-Path $Artifact).Path
$manifestDirectory = Join-Path $repository "platforms/windows/arm64x"
$expectedFiles = @(
    "SlimeIME.dll",
    "SlimeIME_arm64.dll",
    "SlimeIME_x64.dll",
    "SlimeIMERegister.exe",
    "SlimeSettings.exe",
    "slime_ffi.dll",
    "slime_ffi_arm64.dll",
    "slime_ffi_x64.dll"
) | Sort-Object

function Get-ForwarderExports([string]$Name) {
    $path = Join-Path $manifestDirectory $Name
    $exports = @(
        Get-Content -LiteralPath $path |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_ -ne "" }
    )
    if ($exports.Count -eq 0 -or
        @($exports | Where-Object { $_ -notmatch '^[A-Za-z_][A-Za-z0-9_]*$' }).Count -ne 0 -or
        @($exports | Sort-Object -Unique).Count -ne $exports.Count) {
        throw "Invalid ARM64X export manifest: $path"
    }
    return $exports
}

function Invoke-Dumpbin([string]$Mode, [string]$Path) {
    $output = @(& $Dumpbin $Mode $Path 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin $Mode failed for $Path`: $($output -join [Environment]::NewLine)"
    }
    return $output
}

function Assert-Arm64X([string]$Path, [string[]]$RequiredExports) {
    $machine = Get-SlimePeMachine $Path
    if ($machine -notin @([UInt16]0x8664, [UInt16]0xAA64)) {
        throw ("Unexpected ARM64X PE machine 0x{0:X4}: {1}" -f $machine, $Path)
    }
    $headers = Invoke-Dumpbin "/headers" $Path
    if (($headers -join "`n") -notmatch '\(ARM64X\)') {
        throw "PE is not ARM64X: $Path"
    }
    $exports = Invoke-Dumpbin "/exports" $Path
    $exportText = $exports -join "`n"
    foreach ($requiredExport in $RequiredExports) {
        if ($exportText -notmatch "(?m)\s$([regex]::Escape($requiredExport))(?:\s|$)") {
            throw "ARM64X forwarder is missing export $requiredExport`: $Path"
        }
    }
}

$root = Get-Item -LiteralPath $artifactPath -Force
$items = @(Get-ChildItem -LiteralPath $root.FullName -Force -Recurse)
$reparsePoints = @($root) + $items | Where-Object {
    ($_.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
}
if ($reparsePoints.Count -ne 0) {
    throw "ARM64X artifact contains a reparse point: $($reparsePoints[0].FullName)"
}
if (@($items | Where-Object { $_.PSIsContainer }).Count -ne 0) {
    throw "ARM64X artifact must be a flat directory"
}
$actualFiles = @($items | ForEach-Object { $_.Name } | Sort-Object)
$difference = @(Compare-Object $expectedFiles $actualFiles)
if ($difference.Count -ne 0) {
    throw "ARM64X artifact file list is unexpected: $($difference | Out-String)"
}

Assert-Arm64X `
    (Join-Path $artifactPath "SlimeIME.dll") `
    (Get-ForwarderExports "slime-ime-exports.txt")
Assert-Arm64X `
    (Join-Path $artifactPath "slime_ffi.dll") `
    (Get-ForwarderExports "slime-ffi-exports.txt")

foreach ($file in @("SlimeIME_arm64.dll", "slime_ffi_arm64.dll",
                    "SlimeIMERegister.exe", "SlimeSettings.exe")) {
    Assert-SlimePeMachine (Join-Path $artifactPath $file) 0xAA64
}
foreach ($file in @("SlimeIME_x64.dll", "slime_ffi_x64.dll")) {
    Assert-SlimePeMachine (Join-Path $artifactPath $file) 0x8664
}
foreach ($file in $expectedFiles) {
    $path = Join-Path $artifactPath $file
    if ($ExpectedSourceRevision) {
        Assert-SlimePeSourceRevision $path $ExpectedSourceRevision
    }
    if ($ExpectedVersion) {
        Assert-SlimePeProductVersion $path $ExpectedVersion
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $path
    if (-not $AllowSigned -and
        $signature.Status -ne [System.Management.Automation.SignatureStatus]::NotSigned) {
        throw "Development ARM64X artifact must be unsigned: $path ($($signature.Status))"
    }
}

Write-Output "Windows ARM64X development artifact verification passed"
