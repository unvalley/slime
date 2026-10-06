$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-signature.ps1")

function New-FixtureCertificate(
    [string[]]$EnhancedKeyUsages,
    [int]$KeySize = 2048
) {
    $rsa = [System.Security.Cryptography.RSA]::Create()
    $rsa.KeySize = $KeySize
    try {
        $request = [System.Security.Cryptography.X509Certificates.CertificateRequest]::new(
            "CN=Slime signature policy fixture",
            $rsa,
            [System.Security.Cryptography.HashAlgorithmName]::SHA256,
            [System.Security.Cryptography.RSASignaturePadding]::Pkcs1
        )
        if ($EnhancedKeyUsages.Count -gt 0) {
            $oids = [System.Security.Cryptography.OidCollection]::new()
            foreach ($oid in $EnhancedKeyUsages) {
                $null = $oids.Add([System.Security.Cryptography.Oid]::new($oid))
            }
            $request.CertificateExtensions.Add(
                [System.Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new(
                    $oids,
                    $false
                )
            )
        }
        return $request.CreateSelfSigned(
            [DateTimeOffset]::UtcNow.AddMinutes(-1),
            [DateTimeOffset]::UtcNow.AddMinutes(1)
        )
    } finally {
        $rsa.Dispose()
    }
}

function Assert-RsaRejected(
    [System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
    [string]$ExpectedMessage,
    [int]$MinimumKeySize = 2048
) {
    $rejected = $false
    try {
        Assert-SlimeCertificateRsaPublicKey `
            $Certificate "Fixture" "fixture" $MinimumKeySize
    } catch {
        if ($_.Exception.Message -notlike "*$ExpectedMessage*") {
            throw "RSA policy failed for an unexpected reason: $($_.Exception.Message)"
        }
        $rejected = $true
    }
    if (-not $rejected) {
        throw "RSA policy unexpectedly accepted the fixture"
    }
}

function Assert-EkuRejected(
    [System.Security.Cryptography.X509Certificates.X509Certificate2]$Certificate,
    [string]$RequiredOid
) {
    $rejected = $false
    try {
        Assert-SlimeCertificateEku $Certificate $RequiredOid "Fixture" "fixture"
    } catch {
        $rejected = $true
    }
    if (-not $rejected) {
        throw "Certificate unexpectedly accepted required EKU $RequiredOid"
    }
}

function Assert-ThumbprintRejected([string]$Actual, [string]$Expected) {
    $rejected = $false
    try {
        Assert-SlimeSignerThumbprint $Actual $Expected "fixture"
    } catch {
        $rejected = $true
    }
    if (-not $rejected) {
        throw "Signer thumbprint policy unexpectedly accepted the fixture"
    }
}

function New-PeFixture([string]$Path, [UInt16]$Machine) {
    $bytes = [byte[]]::new(0x46)
    $bytes[0] = 0x4D
    $bytes[1] = 0x5A
    [BitConverter]::GetBytes([UInt32]0x40).CopyTo($bytes, 0x3C)
    $bytes[0x40] = 0x50
    $bytes[0x41] = 0x45
    [BitConverter]::GetBytes($Machine).CopyTo($bytes, 0x44)
    [System.IO.File]::WriteAllBytes($Path, $bytes)
}

function Assert-PeRejected(
    [string]$Path,
    [UInt16]$ExpectedMachine,
    [string]$ExpectedMessage
) {
    $rejected = $false
    try {
        Assert-SlimePeMachine $Path $ExpectedMachine
    } catch {
        if ($_.Exception.Message -notlike "*$ExpectedMessage*") {
            throw "PE policy failed for an unexpected reason: $($_.Exception.Message)"
        }
        $rejected = $true
    }
    if (-not $rejected) {
        throw "PE policy unexpectedly accepted the fixture"
    }
}

function Assert-SourceRevisionRejected(
    [string]$Path,
    [string]$ExpectedRevision,
    [string]$ExpectedMessage
) {
    $rejected = $false
    try {
        Assert-SlimePeSourceRevision $Path $ExpectedRevision
    } catch {
        if ($_.Exception.Message -notlike "*$ExpectedMessage*") {
            throw "Source revision policy failed for an unexpected reason: $($_.Exception.Message)"
        }
        $rejected = $true
    }
    if (-not $rejected) {
        throw "Source revision policy unexpectedly accepted the fixture"
    }
}

function Assert-ProductVersionRejected(
    [string]$Path,
    [string]$ExpectedVersion,
    [string]$ExpectedMessage
) {
    $rejected = $false
    try {
        Assert-SlimePeProductVersion $Path $ExpectedVersion
    } catch {
        if ($_.Exception.Message -notlike "*$ExpectedMessage*") {
            throw "Product version policy failed for an unexpected reason: $($_.Exception.Message)"
        }
        $rejected = $true
    }
    if (-not $rejected) {
        throw "Product version policy unexpectedly accepted the fixture"
    }
}

$codeSigningOid = "1.3.6.1.5.5.7.3.3"
$timestampingOid = "1.3.6.1.5.5.7.3.8"
$codeSigning = New-FixtureCertificate @($codeSigningOid)
$timestamping = New-FixtureCertificate @($timestampingOid)
$noEku = New-FixtureCertificate @()
$weakCodeSigning = New-FixtureCertificate @($codeSigningOid) 1024
try {
    Assert-SlimeCertificateEku $codeSigning $codeSigningOid "Code signing" "fixture"
    Assert-SlimeCertificateRsaPublicKey $codeSigning "Code signing" "fixture"
    Assert-RsaRejected $weakCodeSigning "smaller than 2048 bits"
    Assert-RsaRejected $codeSigning "below the release policy" -MinimumKeySize 1024
    Assert-EkuRejected $codeSigning $timestampingOid
    Assert-SlimeCertificateEku $timestamping $timestampingOid "Timestamping" "fixture"
    Assert-EkuRejected $timestamping $codeSigningOid
    Assert-EkuRejected $noEku $codeSigningOid
    $codeSigningThumbprint = $codeSigning.Thumbprint
    Assert-SlimeSignerThumbprint `
        $codeSigningThumbprint `
        $codeSigningThumbprint.ToLowerInvariant() `
        "fixture"
    Assert-ThumbprintRejected `
        $codeSigningThumbprint `
        ("0" * $codeSigningThumbprint.Length)
    Assert-ThumbprintRejected $codeSigningThumbprint "not-a-thumbprint"
} finally {
    $codeSigning.Dispose()
    $timestamping.Dispose()
    $noEku.Dispose()
    $weakCodeSigning.Dispose()
}

$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) `
    "slime-pe-policy-$([guid]::NewGuid())"
try {
    New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
    $x64Fixture = Join-Path $temporaryDirectory "x64.exe"
    $x86Fixture = Join-Path $temporaryDirectory "x86.exe"
    $invalidFixture = Join-Path $temporaryDirectory "invalid.exe"
    New-PeFixture $x64Fixture 0x8664
    New-PeFixture $x86Fixture 0x014C
    [System.IO.File]::WriteAllText($invalidFixture, "not a PE")
    Assert-SlimePeMachine $x64Fixture 0x8664
    Assert-SlimePeMachine $x86Fixture 0x014C
    Assert-PeRejected $x64Fixture 0x014C "Unexpected PE machine"
    Assert-PeRejected $x86Fixture 0x8664 "Unexpected PE machine"
    Assert-PeRejected $invalidFixture 0x8664 "truncated"
    Assert-SourceRevisionRejected $x64Fixture "not-a-revision" "malformed"
    Assert-SourceRevisionRejected $x64Fixture ("0" * 40) "does not match"
    Assert-ProductVersionRejected $x64Fixture "not-a-version" "malformed"
    Assert-ProductVersionRejected $x64Fixture "0.1.0" "does not match"
} finally {
    if (Test-Path -LiteralPath $temporaryDirectory) {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
    }
}

Write-Output "Windows signature and PE architecture policy tests passed"
