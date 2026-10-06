function Get-SlimeWindowsConsumerEvidenceSteps {
    return @(
        "download-reputation",
        "desktop-input",
        "candidate-interaction",
        "search-integration",
        "accessibility",
        "settings-reload"
    )
}

function Assert-SlimeWindowsConsumerBinding([object]$Binding) {
    if (-not $Binding) {
        throw "Consumer evidence binding is missing"
    }
    if ($Binding.Architecture -notin @("x64", "arm64x")) {
        throw "Consumer evidence architecture is invalid"
    }
    if ($Binding.Version -notmatch '^\d+\.\d+\.\d+$') {
        throw "Consumer evidence version is invalid"
    }
    if ($Binding.SourceRevision -notmatch '^[0-9a-f]{40}$') {
        throw "Consumer evidence source revision is invalid"
    }
    if ($Binding.SignerThumbprint -notmatch '^[0-9A-F]{40}$') {
        throw "Consumer evidence signer thumbprint is invalid"
    }
    foreach ($name in @("InstallerSha256", "InstalledManifestSha256")) {
        if ($Binding.$name -notmatch '^[0-9A-F]{64}$') {
            throw "Consumer evidence $name is invalid"
        }
    }
}

function ConvertTo-SlimeWindowsReleaseBindingJson([object]$Binding) {
    Assert-SlimeWindowsConsumerBinding $Binding
    $canonical = [ordered]@{
        Schema = 1
        Architecture = $Binding.Architecture
        Version = $Binding.Version
        SourceRevision = $Binding.SourceRevision
        SignerThumbprint = $Binding.SignerThumbprint
        InstallerSha256 = $Binding.InstallerSha256
        InstalledManifestSha256 = $Binding.InstalledManifestSha256
    }
    return ($canonical | ConvertTo-Json -Compress)
}

function Assert-SlimeWindowsConsumerSessionBinding([object]$Binding) {
    if (-not $Binding) {
        throw "Consumer evidence session binding is missing"
    }
    if ($Binding.UserSid -notmatch '^S-1-(\d+-){1,14}\d+$') {
        throw "Consumer evidence user SID is invalid"
    }
    if ("$($Binding.SessionId)" -notmatch '^\d{1,10}$') {
        throw "Consumer evidence session ID is invalid"
    }
    try {
        $null = [datetimeoffset]::Parse(
            $Binding.BootTimeUtc,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind
        )
    } catch {
        throw "Consumer evidence boot time is invalid"
    }
}

function Assert-SlimeWindowsConsumerEvidenceState(
    [object]$State,
    [datetimeoffset]$Now = [datetimeoffset]::UtcNow,
    [switch]$RequireComplete
) {
    if (-not $State -or $State.Schema -ne 2) {
        throw "Consumer evidence schema is invalid"
    }
    if ($State.Token -notmatch '^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$') {
        throw "Consumer evidence token is invalid"
    }
    Assert-SlimeWindowsConsumerBinding $State.Binding
    Assert-SlimeWindowsConsumerSessionBinding $State.SessionBinding

    try {
        $preparedAt = [datetimeoffset]::Parse(
            $State.PreparedAtUtc,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind
        )
        $expiresAt = [datetimeoffset]::Parse(
            $State.ExpiresAtUtc,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind
        )
        $bootTime = [datetimeoffset]::Parse(
            $State.SessionBinding.BootTimeUtc,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind
        )
    } catch {
        throw "Consumer evidence timestamps are invalid"
    }
    if ($expiresAt -le $preparedAt -or $expiresAt -gt $preparedAt.AddHours(4)) {
        throw "Consumer evidence validity window is invalid"
    }
    if ($preparedAt -lt $bootTime) {
        throw "Consumer evidence was prepared before the bound boot session"
    }
    if ($Now -lt $preparedAt.AddMinutes(-1) -or $Now -gt $expiresAt) {
        throw "Consumer evidence session has expired or is not active"
    }

    $required = @(Get-SlimeWindowsConsumerEvidenceSteps)
    $completed = @($State.CompletedSteps)
    if (@($completed | Sort-Object -Unique).Count -ne $completed.Count) {
        throw "Consumer evidence contains duplicate completed steps"
    }
    if (@($completed | Where-Object { $_ -notin $required }).Count -ne 0) {
        throw "Consumer evidence contains an unknown completed step"
    }
    $orderedCompleted = @($required | Where-Object { $_ -in $completed })
    if (@(Compare-Object $orderedCompleted $completed -SyncWindow 0).Count -ne 0) {
        throw "Consumer evidence completed steps are out of order"
    }
    if ($RequireComplete -and $completed.Count -ne $required.Count) {
        $missing = @($required | Where-Object { $_ -notin $completed })
        throw "Consumer evidence is incomplete: $($missing -join ', ')"
    }
}

function New-SlimeWindowsConsumerEvidenceState(
    [object]$Binding,
    [object]$SessionBinding,
    [datetimeoffset]$Now = [datetimeoffset]::UtcNow,
    [guid]$Token = [guid]::NewGuid()
) {
    Assert-SlimeWindowsConsumerBinding $Binding
    Assert-SlimeWindowsConsumerSessionBinding $SessionBinding
    $state = [ordered]@{
        Schema = 2
        Token = $Token.ToString().ToLowerInvariant()
        PreparedAtUtc = $Now.ToUniversalTime().ToString("o")
        ExpiresAtUtc = $Now.ToUniversalTime().AddHours(2).ToString("o")
        Binding = $Binding
        SessionBinding = $SessionBinding
        CompletedSteps = @()
    }
    Assert-SlimeWindowsConsumerEvidenceState ([PSCustomObject]$state) -Now $Now
    return [PSCustomObject]$state
}

function Assert-SlimeWindowsConsumerSessionMatches(
    [object]$Expected,
    [object]$Actual
) {
    Assert-SlimeWindowsConsumerSessionBinding $Expected
    Assert-SlimeWindowsConsumerSessionBinding $Actual
    foreach ($name in @("UserSid", "SessionId", "BootTimeUtc")) {
        if ("$($Expected.$name)" -cne "$($Actual.$name)") {
            throw "Consumer evidence interactive session changed: $name"
        }
    }
}

function Add-SlimeWindowsConsumerEvidenceStep(
    [object]$State,
    [string]$Step,
    [string]$Token,
    [datetimeoffset]$Now = [datetimeoffset]::UtcNow
) {
    Assert-SlimeWindowsConsumerEvidenceState $State -Now $Now
    if ($Token -cne $State.Token) {
        throw "Consumer evidence token does not match the prepared session"
    }
    $required = @(Get-SlimeWindowsConsumerEvidenceSteps)
    $nextIndex = @($State.CompletedSteps).Count
    if ($nextIndex -ge $required.Count) {
        throw "Consumer evidence is already complete"
    }
    if ($Step -ne $required[$nextIndex]) {
        throw "Consumer evidence step '$Step' is out of order; expected '$($required[$nextIndex])'"
    }
    $State.CompletedSteps = @($State.CompletedSteps) + $Step
    Assert-SlimeWindowsConsumerEvidenceState $State -Now $Now
    return $State
}

function Assert-SlimeWindowsConsumerBindingMatches(
    [object]$Expected,
    [object]$Actual
) {
    Assert-SlimeWindowsConsumerBinding $Expected
    Assert-SlimeWindowsConsumerBinding $Actual
    foreach ($name in @(
        "Architecture",
        "Version",
        "SourceRevision",
        "SignerThumbprint",
        "InstallerSha256",
        "InstalledManifestSha256"
    )) {
        if ($Expected.$name -cne $Actual.$name) {
            throw "Consumer evidence artifact binding changed: $name"
        }
    }
}
