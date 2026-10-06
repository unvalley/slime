function Get-SlimeCleanReleaseSourceRevision([string]$Repository) {
    $repositoryPath = (Resolve-Path $Repository).Path
    $repositoryRoot = (& git -C $repositoryPath rev-parse --show-toplevel 2>$null)
    if ($LASTEXITCODE -ne 0 -or -not $repositoryRoot) {
        throw "Windows release builds require a Git checkout"
    }
    $repositoryPrefixOutput = @(
        & git -C $repositoryPath rev-parse --show-prefix 2>$null
    )
    if ($LASTEXITCODE -ne 0) {
        throw "Windows release build directory could not be resolved"
    }
    $repositoryPrefix = ($repositoryPrefixOutput -join "").Trim()
    if (-not [string]::IsNullOrEmpty($repositoryPrefix)) {
        throw "Windows release build directory must be the repository root"
    }
    $repositoryRoot = (Resolve-Path $repositoryRoot.Trim()).Path

    $revisionOutput = (& git -C $repositoryRoot rev-parse --verify 'HEAD^{commit}' 2>$null)
    if ($LASTEXITCODE -ne 0 -or -not $revisionOutput) {
        throw "Windows release builds require a committed source revision"
    }
    $revision = $revisionOutput.Trim()
    if ($revision -notmatch '^[0-9a-f]{40}$') {
        throw "Windows release source revision is malformed"
    }

    $status = @(& git -C $repositoryRoot status --porcelain=v1 --untracked-files=all)
    if ($LASTEXITCODE -ne 0) {
        throw "Windows release source status could not be inspected"
    }
    if ($status.Count -ne 0) {
        throw "Windows release builds require a clean source checkout"
    }

    return $revision
}
