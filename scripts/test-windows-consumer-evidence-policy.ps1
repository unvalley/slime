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
    throw "Policy unexpectedly accepted invalid consumer evidence"
}

$now = [datetimeoffset]::Parse("2026-08-08T00:00:00Z")
$binding = [PSCustomObject]@{
    Architecture = "x64"
    Version = "1.2.3"
    SourceRevision = "a" * 40
    SignerThumbprint = "B" * 40
    InstallerSha256 = "C" * 64
    InstalledManifestSha256 = "D" * 64
}
$sessionBinding = [PSCustomObject]@{
    UserSid = "S-1-5-21-1000-1001-1002-1003"
    SessionId = 3
    BootTimeUtc = "2026-08-07T23:00:00.0000000+00:00"
}
$token = [guid]::Parse("12345678-1234-4123-8123-123456789abc")
$state = New-SlimeWindowsConsumerEvidenceState `
    $binding $sessionBinding -Now $now -Token $token
Assert-SlimeWindowsConsumerEvidenceState $state -Now $now
$bindingJson = ConvertTo-SlimeWindowsReleaseBindingJson $binding
$decodedBinding = $bindingJson | ConvertFrom-Json
if ($decodedBinding.Schema -ne 1 -or
    $decodedBinding.Architecture -ne $binding.Architecture -or
    $decodedBinding.InstalledManifestSha256 -ne $binding.InstalledManifestSha256) {
    throw "Canonical Windows release binding JSON is inconsistent"
}

Assert-Rejected {
    Add-SlimeWindowsConsumerEvidenceStep `
        $state "candidate-interaction" $state.Token -Now $now
} "out of order"
Assert-Rejected {
    Add-SlimeWindowsConsumerEvidenceStep `
        $state "desktop-input" "00000000-0000-4000-8000-000000000000" -Now $now
} "token does not match"

foreach ($step in Get-SlimeWindowsConsumerEvidenceSteps) {
    $state = Add-SlimeWindowsConsumerEvidenceStep $state $step $state.Token -Now $now
}
Assert-SlimeWindowsConsumerEvidenceState $state -Now $now -RequireComplete

$incomplete = New-SlimeWindowsConsumerEvidenceState `
    $binding $sessionBinding -Now $now -Token $token
Assert-Rejected {
    Assert-SlimeWindowsConsumerEvidenceState $incomplete -Now $now -RequireComplete
} "incomplete"
Assert-Rejected {
    Assert-SlimeWindowsConsumerEvidenceState $state -Now $now.AddHours(3)
} "expired"

$changed = $binding.PSObject.Copy()
$changed.InstallerSha256 = "E" * 64
Assert-Rejected {
    Assert-SlimeWindowsConsumerBindingMatches $binding $changed
} "binding changed"
if ((ConvertTo-SlimeWindowsReleaseBindingJson $changed) -eq $bindingJson) {
    throw "Canonical Windows release binding JSON ignored an artifact change"
}

$malformed = $binding.PSObject.Copy()
$malformed.SignerThumbprint = "not-a-thumbprint"
Assert-Rejected {
    Assert-SlimeWindowsConsumerBinding $malformed
} "thumbprint is invalid"

$changedSession = $sessionBinding.PSObject.Copy()
$changedSession.SessionId = 4
Assert-Rejected {
    Assert-SlimeWindowsConsumerSessionMatches $sessionBinding $changedSession
} "interactive session changed"

$malformedSession = $sessionBinding.PSObject.Copy()
$malformedSession.UserSid = "not-a-sid"
Assert-Rejected {
    Assert-SlimeWindowsConsumerSessionBinding $malformedSession
} "user SID is invalid"

Write-Output "Windows consumer evidence policy test passed"
