$ErrorActionPreference = "Stop"
$thumbprint = "0" * 40
$revision = "0" * 40

foreach ($script in @(
    "verify-windows-release.ps1",
    "test-windows-install-lifecycle.ps1"
)) {
    $scriptPath = Join-Path $PSScriptRoot $script
    foreach ($nativeArguments in @(
        @{},
        @{ PayloadX64 = $PSScriptRoot; PayloadARM64X = $PSScriptRoot }
    )) {
        $parameters = @{
            Version = "0.1.0"
            PayloadX86 = $PSScriptRoot
            Installer = $scriptPath
            ExpectedSignerThumbprint = $thumbprint
            ExpectedSourceRevision = $revision
        }
        foreach ($key in $nativeArguments.Keys) {
            $parameters[$key] = $nativeArguments[$key]
        }
        try {
            & $scriptPath @parameters
        } catch {
            if ($_.Exception.Message -notlike "*Specify exactly one*") {
                throw "Architecture policy failed for an unexpected reason: $($_.Exception.Message)"
            }
            continue
        }
        throw "Architecture policy accepted an ambiguous release payload: $script"
    }
}

Write-Output "Windows release architecture selection policy passed"
