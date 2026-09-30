param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Prepare", "Record", "Check", "Cancel")]
    [string]$Action,

    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [string]$PayloadX64,

    [string]$PayloadARM64X,

    [string]$PayloadX86,

    [string]$Installer,

    [ValidatePattern('^[0-9A-Fa-f]{40}$')]
    [string]$ExpectedSignerThumbprint,

    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$ExpectedSourceRevision,

    [ValidateSet(
        "download-reputation",
        "desktop-input",
        "candidate-interaction",
        "search-integration",
        "accessibility",
        "settings-reload"
    )]
    [string]$Step,

    [string]$Token,

    [string]$StatePath = (Join-Path $PSScriptRoot "../target/windows-consumer-input-gate.json")
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-signature.ps1")
. (Join-Path $PSScriptRoot "windows-install-layout.ps1")
. (Join-Path $PSScriptRoot "windows-consumer-evidence.ps1")

$productKey = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\SlimeIME"
$utf8WithoutBom = New-Object System.Text.UTF8Encoding($false)

function Assert-ReleaseTestSession {
    if ($env:SLIME_RELEASE_TEST_VM -ne "1") {
        throw "Run only in a disposable Windows VM with SLIME_RELEASE_TEST_VM=1"
    }
    if (-not [Environment]::UserInteractive) {
        throw "The consumer input gate requires an interactive Windows session"
    }
}

function Assert-ReleaseInputs {
    foreach ($value in @(
        $Version,
        $PayloadX86,
        $Installer,
        $ExpectedSignerThumbprint,
        $ExpectedSourceRevision
    )) {
        if ([string]::IsNullOrWhiteSpace($value)) {
            throw "Release artifact parameters are required for $Action"
        }
    }
    $hasX64 = -not [string]::IsNullOrWhiteSpace($PayloadX64)
    $hasARM64X = -not [string]::IsNullOrWhiteSpace($PayloadARM64X)
    if ($hasX64 -eq $hasARM64X) {
        throw "Specify exactly one of -PayloadX64 or -PayloadARM64X"
    }
}

function Get-ConsumerSessionBinding {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $identity.User) {
        throw "Cannot determine the interactive Windows user SID"
    }
    $operatingSystem = Get-CimInstance -ClassName Win32_OperatingSystem
    if (-not $operatingSystem.LastBootUpTime) {
        throw "Cannot determine the Windows boot time"
    }
    return [PSCustomObject][ordered]@{
        UserSid = $identity.User.Value
        SessionId = [System.Diagnostics.Process]::GetCurrentProcess().SessionId
        BootTimeUtc = ([datetimeoffset]$operatingSystem.LastBootUpTime).ToUniversalTime().ToString("o")
    }
}

function Get-ReleaseBinding {
    Assert-ReleaseInputs
    $hasARM64X = -not [string]::IsNullOrWhiteSpace($PayloadARM64X)
    $nativeDirectory = if ($hasARM64X) { "arm64x" } else { "x64" }
    $nativePayload = if ($hasARM64X) {
        (Resolve-Path $PayloadARM64X).Path
    } else {
        (Resolve-Path $PayloadX64).Path
    }
    $x86Payload = (Resolve-Path $PayloadX86).Path
    $installerPath = (Resolve-Path $Installer).Path
    $expectedSigner = $ExpectedSignerThumbprint.ToUpperInvariant()

    $verification = @{
        Version = $Version
        PayloadX86 = $x86Payload
        Installer = $installerPath
        ExpectedSignerThumbprint = $expectedSigner
        ExpectedSourceRevision = $ExpectedSourceRevision
    }
    if ($hasARM64X) {
        $verification.PayloadARM64X = $nativePayload
    } else {
        $verification.PayloadX64 = $nativePayload
    }
    & (Join-Path $PSScriptRoot "verify-windows-release.ps1") @verification | Out-Null

    if (-not (Test-Path $productKey)) {
        throw "The signed release is not registered for uninstall"
    }
    $properties = Get-ItemProperty $productKey
    $installDirectory = $properties.InstallLocation
    if ([string]::IsNullOrWhiteSpace($installDirectory) -or
        -not (Test-Path -LiteralPath $installDirectory -PathType Container)) {
        throw "The installed release directory is missing"
    }
    $expectedDirectory = Join-Path $env:ProgramFiles "Slime\$Version"
    if ([IO.Path]::GetFullPath($installDirectory) -ne [IO.Path]::GetFullPath($expectedDirectory)) {
        throw "The installed release directory does not match the expected version"
    }
    Assert-SlimeInstalledLayout $installDirectory $nativeDirectory
    Assert-SlimeUninstallRegistration `
        $properties $Version $installDirectory $nativeDirectory $ExpectedSourceRevision

    foreach ($architecture in @(
        @{
            Name = $nativeDirectory
            Source = $nativePayload
            Files = @(Get-SlimeNativePayloadFiles $nativeDirectory)
        },
        @{
            Name = "x86"
            Source = $x86Payload
            Files = @(Get-SlimeNativePayloadFiles "x64")
        }
    )) {
        foreach ($file in $architecture.Files) {
            $source = Join-Path $architecture.Source $file
            $installed = Join-Path $installDirectory "$($architecture.Name)\$file"
            if ((Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash -ne
                (Get-FileHash -Algorithm SHA256 -LiteralPath $installed).Hash) {
                throw "Installed release differs from the verified payload: $($architecture.Name)\$file"
            }
            $signature = Assert-SlimeAuthenticodeSignature $installed
            Assert-SlimeSignerThumbprint `
                $signature.SignerCertificate.Thumbprint $expectedSigner $installed
        }
    }

    $uninstaller = Join-Path $installDirectory "Uninstall.exe"
    $uninstallerSignature = Assert-SlimeAuthenticodeSignature $uninstaller
    Assert-SlimeSignerThumbprint `
        $uninstallerSignature.SignerCertificate.Thumbprint $expectedSigner $uninstaller

    return [PSCustomObject]@{
        Architecture = $nativeDirectory
        Version = $Version
        SourceRevision = $ExpectedSourceRevision
        SignerThumbprint = $expectedSigner
        InstallerSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $installerPath).Hash
        InstalledManifestSha256 = Get-SlimeInstalledManifestSha256 `
            $installDirectory $nativeDirectory
    }
}

function Assert-StatePathSafe([switch]$AllowMissing) {
    $absolutePath = [IO.Path]::GetFullPath($StatePath)
    if (Test-Path -LiteralPath $absolutePath) {
        $item = Get-Item -LiteralPath $absolutePath -Force
        if ($item.PSIsContainer -or
            ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Consumer gate state path is not a regular file"
        }
    } elseif (-not $AllowMissing) {
        throw "No prepared Windows consumer input gate was found"
    }
    return $absolutePath
}

function Read-State {
    $path = Assert-StatePathSafe
    if ((Get-Item -LiteralPath $path).Length -gt 65536) {
        throw "Consumer gate state exceeds the size limit"
    }
    return Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
}

function Write-State([object]$State, [switch]$CreateOnly) {
    $path = Assert-StatePathSafe -AllowMissing
    if ($CreateOnly -and (Test-Path -LiteralPath $path)) {
        throw "A Windows consumer input gate is already prepared"
    }
    $directory = Split-Path -Parent $path
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $temporary = "$path.$([guid]::NewGuid().ToString('N')).tmp"
    try {
        [IO.File]::WriteAllText(
            $temporary,
            (($State | ConvertTo-Json -Depth 5) + "`n"),
            $utf8WithoutBom
        )
        Move-Item -LiteralPath $temporary -Destination $path -Force
    } finally {
        if (Test-Path -LiteralPath $temporary) {
            Remove-Item -LiteralPath $temporary -Force
        }
    }
}

switch ($Action) {
    "Prepare" {
        Assert-ReleaseTestSession
        $path = Assert-StatePathSafe -AllowMissing
        if (Test-Path -LiteralPath $path) {
            throw "A Windows consumer input gate is already prepared"
        }
        $binding = Get-ReleaseBinding
        $sessionBinding = Get-ConsumerSessionBinding
        $state = New-SlimeWindowsConsumerEvidenceState $binding $sessionBinding
        Write-State $state -CreateOnly
        Write-Output "Windows consumer input gate prepared"
        Write-Output "Verification token: $($state.Token)"
        Write-Output "Complete and record these steps in order:"
        foreach ($requiredStep in Get-SlimeWindowsConsumerEvidenceSteps) {
            Write-Output "  $requiredStep"
        }
        Write-Output "Record a step only after observing it in the interactive consumer session."
    }
    "Record" {
        Assert-ReleaseTestSession
        if ([string]::IsNullOrWhiteSpace($Step) -or [string]::IsNullOrWhiteSpace($Token)) {
            throw "Record requires -Step and -Token"
        }
        $state = Read-State
        Assert-SlimeWindowsConsumerEvidenceState $state
        Assert-SlimeWindowsConsumerSessionMatches `
            $state.SessionBinding (Get-ConsumerSessionBinding)
        $currentBinding = Get-ReleaseBinding
        Assert-SlimeWindowsConsumerBindingMatches $state.Binding $currentBinding
        $state = Add-SlimeWindowsConsumerEvidenceStep $state $Step $Token
        Write-State $state
        Write-Output "Recorded Windows consumer evidence step: $Step"
    }
    "Check" {
        Assert-ReleaseTestSession
        $state = Read-State
        Assert-SlimeWindowsConsumerEvidenceState $state -RequireComplete
        Assert-SlimeWindowsConsumerSessionMatches `
            $state.SessionBinding (Get-ConsumerSessionBinding)
        $currentBinding = Get-ReleaseBinding
        Assert-SlimeWindowsConsumerBindingMatches $state.Binding $currentBinding
        $path = Assert-StatePathSafe
        Remove-Item -LiteralPath $path -Force
        Write-Output "Windows signed consumer input and accessibility gate passed"
        Write-Output "Architecture: $($currentBinding.Architecture)"
        Write-Output "Version: $($currentBinding.Version)"
        Write-Output "Signer thumbprint: $($currentBinding.SignerThumbprint)"
        Write-Output "Prepared at UTC: $($state.PreparedAtUtc)"
        Write-Output "Completed at UTC: $([datetimeoffset]::UtcNow.ToString('o'))"
        Write-Output "Verified steps: $($state.CompletedSteps -join ', ')"
        Write-Output "Source revision: $($currentBinding.SourceRevision)"
        Write-Output "Release installer SHA-256: $($currentBinding.InstallerSha256)"
        Write-Output "Installed manifest SHA-256: $($currentBinding.InstalledManifestSha256)"
        $releaseBindingJson = ConvertTo-SlimeWindowsReleaseBindingJson $currentBinding
        Write-Output "Release binding JSON: $releaseBindingJson"
    }
    "Cancel" {
        $path = Assert-StatePathSafe -AllowMissing
        if (Test-Path -LiteralPath $path) {
            Remove-Item -LiteralPath $path -Force
        }
        Write-Output "Windows consumer input gate cancelled"
    }
}
