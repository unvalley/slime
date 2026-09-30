function Assert-SlimeCertificateEku(
    [System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
    [string]$RequiredOid,
    [string]$Purpose,
    [string]$Path
) {
    $extension = $Certificate.Extensions |
        Where-Object { $_.Oid.Value -eq "2.5.29.37" } |
        Select-Object -First 1
    if (-not $extension) {
        throw "$Purpose certificate has no enhanced key usage extension: $Path"
    }
    $enhancedKeyUsage = [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]$extension
    $matches = @($enhancedKeyUsage.EnhancedKeyUsages | Where-Object { $_.Value -eq $RequiredOid })
    if ($matches.Count -eq 0) {
        throw "$Purpose certificate is not valid for the required usage $RequiredOid`: $Path"
    }
}

function Assert-SlimeCertificateRsaPublicKey(
    [System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
    [string]$Purpose,
    [string]$Path,
    [int]$MinimumKeySize = 2048
) {
    if ($MinimumKeySize -lt 2048) {
        throw "Minimum RSA key size is below the release policy: $Path"
    }
    $rsa = [System.Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPublicKey(
        $Certificate
    )
    if (-not $rsa) {
        throw "$Purpose certificate does not use RSA: $Path"
    }
    try {
        if ($rsa.KeySize -lt $MinimumKeySize) {
            throw "$Purpose certificate RSA key is smaller than $MinimumKeySize bits: $Path"
        }
    } finally {
        $rsa.Dispose()
    }
}

function Assert-SlimeAuthenticodeSignature([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Signed PE is missing: $Path"
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Authenticode signature is not valid for $Path`: $($signature.Status)"
    }
    if (-not $signature.SignerCertificate -or -not $signature.TimeStamperCertificate) {
        throw "Signer or timestamp certificate is missing for $Path"
    }
    Assert-SlimeCertificateEku `
        $signature.SignerCertificate `
        "1.3.6.1.5.5.7.3.3" `
        "Code signing" `
        $Path
    Assert-SlimeCertificateRsaPublicKey `
        $signature.SignerCertificate `
        "Code signing" `
        $Path
    Assert-SlimeCertificateEku `
        $signature.TimeStamperCertificate `
        "1.3.6.1.5.5.7.3.8" `
        "Timestamping" `
        $Path
    return $signature
}

function Assert-SlimeSignerThumbprint(
    [string]$ActualThumbprint,
    [string]$ExpectedThumbprint,
    [string]$Path
) {
    if ($ExpectedThumbprint -notmatch '^[0-9A-Fa-f]{40}$') {
        throw "Expected signer thumbprint is malformed: $Path"
    }
    if ($ActualThumbprint -ne $ExpectedThumbprint.ToUpperInvariant()) {
        throw "Signer certificate does not match the expected thumbprint: $Path"
    }
}

function Assert-SlimePeSourceRevision(
    [string]$Path,
    [string]$ExpectedSourceRevision
) {
    if ($ExpectedSourceRevision -notmatch '^[0-9a-f]{40}$') {
        throw "Expected source revision is malformed: $Path"
    }
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "PE file is missing: $Path"
    }
    $comments = (Get-Item -LiteralPath $Path).VersionInfo.Comments
    if ($comments -ne "Source revision: $ExpectedSourceRevision") {
        throw "PE source revision metadata does not match $ExpectedSourceRevision`: $Path"
    }
}

function Assert-SlimePeProductVersion(
    [string]$Path,
    [string]$ExpectedVersion
) {
    if ($ExpectedVersion -notmatch '^\d+\.\d+\.\d+$') {
        throw "Expected product version is malformed: $Path"
    }
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "PE file is missing: $Path"
    }
    $version = (Get-Item -LiteralPath $Path).VersionInfo.ProductVersion
    if (-not $version -or $version.Trim() -ne $ExpectedVersion) {
        throw "PE product version metadata does not match $ExpectedVersion`: $Path"
    }
}

function Get-SlimePeMachine([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "PE file is missing: $Path"
    }
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        if ($stream.Length -lt 0x46) {
            throw "PE file is truncated: $Path"
        }
        $reader = [System.IO.BinaryReader]::new($stream)
        if ($reader.ReadUInt16() -ne 0x5A4D) {
            throw "Not a PE file: $Path"
        }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        if ($peOffset -gt $stream.Length - 6) {
            throw "PE header is outside the file: $Path"
        }
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "Invalid PE signature: $Path"
        }
        return $reader.ReadUInt16()
    } finally {
        $stream.Dispose()
    }
}

function Assert-SlimePeMachine(
    [string]$Path,
    [UInt16]$ExpectedMachine
) {
    $machine = Get-SlimePeMachine $Path
    if ($machine -ne $ExpectedMachine) {
        throw ("Unexpected PE machine 0x{0:X4} for {1}; expected 0x{2:X4}" -f `
            $machine, $Path, $ExpectedMachine)
    }
}
