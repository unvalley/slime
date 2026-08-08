param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$PayloadX64,

    [Parameter(Mandatory = $true)]
    [string]$PayloadX86,

    [Parameter(Mandatory = $true)]
    [string]$Output,

    [string]$Makensis = "makensis.exe"
)

$ErrorActionPreference = "Stop"
$makensisCommand = Get-Command $Makensis -ErrorAction SilentlyContinue
if ($makensisCommand) {
    $Makensis = $makensisCommand.Source
} elseif ($Makensis -eq "makensis.exe") {
    $installedMakensis = Join-Path ${env:ProgramFiles(x86)} "NSIS/makensis.exe"
    if (-not (Test-Path -LiteralPath $installedMakensis -PathType Leaf)) {
        throw "makensis.exe was not found on PATH or in the standard NSIS installation"
    }
    $Makensis = $installedMakensis
} else {
    throw "makensis executable was not found: $Makensis"
}
$repository = Split-Path -Parent $PSScriptRoot
$installerSource = Join-Path $repository "platforms/windows/installer/Slime.nsi"
$x64 = (Resolve-Path $PayloadX64).Path
$x86 = (Resolve-Path $PayloadX86).Path
$outputDirectory = Split-Path -Parent $Output
if (-not $outputDirectory) {
    $outputDirectory = "."
}
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
$absoluteOutput = [System.IO.Path]::GetFullPath($Output)

$requiredFiles = @(
    "SlimeIME.dll",
    "slime_ffi.dll",
    "SlimeIMERegister.exe",
    "SlimeSettings.exe"
)
foreach ($architecture in @($x64, $x86)) {
    foreach ($file in $requiredFiles) {
        $candidate = Join-Path $architecture $file
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "Missing installer payload: $candidate"
        }
    }
}

& $Makensis `
    "-WX" `
    "/DVERSION=$Version" `
    "/DVERSION_QUAD=$Version.0" `
    "/DPAYLOAD_X64=$x64" `
    "/DPAYLOAD_X86=$x86" `
    "/DOUTPUT=$absoluteOutput" `
    $installerSource
if ($LASTEXITCODE -ne 0) {
    throw "makensis failed with exit code $LASTEXITCODE"
}
if (-not (Test-Path -LiteralPath $absoluteOutput -PathType Leaf)) {
    throw "Installer was not produced: $absoluteOutput"
}

$signature = Get-AuthenticodeSignature -LiteralPath $absoluteOutput
if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::NotSigned) {
    throw "Development installer must be unsigned before the signing stage; status: $($signature.Status)"
}
Get-FileHash -Algorithm SHA256 -LiteralPath $absoluteOutput
