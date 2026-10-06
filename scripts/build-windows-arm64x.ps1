param(
    [Parameter(Mandatory = $true)]
    [string]$PayloadX64,

    [Parameter(Mandatory = $true)]
    [string]$PayloadARM64,

    [Parameter(Mandatory = $true)]
    [string]$Output,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$SourceRevision
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "windows-signature.ps1")
$repository = Split-Path -Parent $PSScriptRoot
$manifestDirectory = Join-Path $repository "platforms/windows/arm64x"
$x64 = (Resolve-Path $PayloadX64).Path
$arm64 = (Resolve-Path $PayloadARM64).Path
$outputPath = [System.IO.Path]::GetFullPath($Output)
$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) `
    "slime-arm64x-$([guid]::NewGuid())"
$stagingDirectory = Join-Path $temporaryDirectory "artifact"

function Invoke-NativeTool([string]$Tool, [string[]]$Arguments) {
    $output = @(& $Tool @Arguments 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "$Tool failed: $($output -join [Environment]::NewLine)"
    }
    return $output
}

function Import-Arm64MsvcEnvironment {
    if ((Get-Command cl.exe -ErrorAction SilentlyContinue) -and
        $env:VSCMD_ARG_TGT_ARCH -eq "arm64") {
        return
    }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} `
        "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        throw "vswhere.exe is required to locate the ARM64 MSVC toolchain"
    }
    $installationPath = (& $vswhere -latest -products * -property installationPath |
        Select-Object -First 1)
    if (-not $installationPath) {
        throw "Visual Studio installation was not found"
    }
    $developerCommand = Join-Path $installationPath "Common7\Tools\VsDevCmd.bat"
    $hostArchitecture = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") {
        "arm64"
    } else {
        "x64"
    }
    $command = "`"$developerCommand`" -no_logo -arch=arm64 " +
        "-host_arch=$hostArchitecture >nul && set"
    $environmentLines = @(& $env:ComSpec /d /s /c $command)
    if ($LASTEXITCODE -ne 0) {
        throw "VsDevCmd.bat failed to initialize the ARM64 toolchain"
    }
    foreach ($line in $environmentLines) {
        $separator = $line.IndexOf("=")
        if ($separator -gt 0) {
            [Environment]::SetEnvironmentVariable(
                $line.Substring(0, $separator),
                $line.Substring($separator + 1),
                [EnvironmentVariableTarget]::Process
            )
        }
    }
    foreach ($tool in @("cl.exe", "link.exe", "dumpbin.exe", "rc.exe")) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "ARM64 MSVC toolchain is missing $tool"
        }
    }
}

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

function Assert-ImplementationExports([string]$Path, [string[]]$RequiredExports) {
    $exports = Invoke-NativeTool "dumpbin.exe" @("/exports", $Path)
    $exportText = $exports -join "`n"
    foreach ($requiredExport in $RequiredExports) {
        if ($exportText -notmatch "(?m)\s$([regex]::Escape($requiredExport))(?:\s|$)") {
            throw "Implementation is missing export $requiredExport`: $Path"
        }
    }
}

function Write-ForwarderDefinition(
    [string]$Path,
    [string]$Library,
    [string]$Implementation,
    [string[]]$Exports
) {
    $lines = @("LIBRARY `"$Library`"", "EXPORTS")
    foreach ($export in $Exports) {
        $lines += "  $export = $Implementation.$export"
    }
    [System.IO.File]::WriteAllLines($Path, $lines)
}

function Build-Forwarder(
    [string]$Library,
    [string]$OutputFile,
    [string]$X64Implementation,
    [string]$Arm64Implementation,
    [string[]]$Exports,
    [string]$Arm64Object,
    [string]$Arm64EcObject,
    [string]$Resource
) {
    $x64Definition = Join-Path $temporaryDirectory "$Library-x64.def"
    $arm64Definition = Join-Path $temporaryDirectory "$Library-arm64.def"
    $x64Import = Join-Path $temporaryDirectory "$Library-x64.lib"
    $arm64Import = Join-Path $temporaryDirectory "$Library-arm64.lib"
    Write-ForwarderDefinition $x64Definition $Library $X64Implementation $Exports
    Write-ForwarderDefinition $arm64Definition $Library $Arm64Implementation $Exports
    Invoke-NativeTool "link.exe" @(
        "/lib", "/machine:x64", "/def:$x64Definition", "/out:$x64Import"
    ) | Out-Null
    Invoke-NativeTool "link.exe" @(
        "/lib", "/machine:arm64", "/def:$arm64Definition", "/out:$arm64Import"
    ) | Out-Null
    Invoke-NativeTool "link.exe" @(
        "/dll", "/noentry", "/machine:arm64x",
        "/defArm64Native:$arm64Definition", "/def:$x64Definition",
        $Arm64Object, $Arm64EcObject, $Resource, "/out:$OutputFile",
        $x64Import, $arm64Import
    ) | Out-Null
}

$requiredPayloadFiles = @(
    "SlimeIME.dll",
    "slime_ffi.dll",
    "SlimeIMERegister.exe",
    "SlimeSettings.exe"
)
$productVersion = $null
foreach ($architecture in @(
    @{ Path = $x64; Machine = [UInt16]0x8664 },
    @{ Path = $arm64; Machine = [UInt16]0xAA64 }
)) {
    foreach ($file in $requiredPayloadFiles) {
        $path = Join-Path $architecture.Path $file
        Assert-SlimePeMachine $path $architecture.Machine
        Assert-SlimePeSourceRevision $path $SourceRevision
        if (-not $productVersion) {
            $candidateVersion = (Get-Item -LiteralPath $path).VersionInfo.ProductVersion
            if (-not $candidateVersion) {
                throw "PE product version metadata is missing: $path"
            }
            $productVersion = $candidateVersion.Trim()
            if ($productVersion -notmatch '^\d+\.\d+\.\d+$') {
                throw "PE product version metadata is malformed: $path"
            }
        }
        Assert-SlimePeProductVersion $path $productVersion
    }
}
$versionParts = @($productVersion.Split(".") | ForEach-Object { [UInt16]$_ })
if (Test-Path -LiteralPath $outputPath) {
    $existing = @(Get-ChildItem -LiteralPath $outputPath -Force)
    if ($existing.Count -ne 0) {
        throw "ARM64X output directory must be empty: $outputPath"
    }
} else {
    New-Item -ItemType Directory -Path $outputPath | Out-Null
}

try {
    New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
    New-Item -ItemType Directory -Path $stagingDirectory | Out-Null
    Import-Arm64MsvcEnvironment
    $imeExports = Get-ForwarderExports "slime-ime-exports.txt"
    $ffiExports = Get-ForwarderExports "slime-ffi-exports.txt"
    Assert-ImplementationExports (Join-Path $x64 "SlimeIME.dll") $imeExports
    Assert-ImplementationExports (Join-Path $arm64 "SlimeIME.dll") $imeExports
    Assert-ImplementationExports (Join-Path $x64 "slime_ffi.dll") $ffiExports
    Assert-ImplementationExports (Join-Path $arm64 "slime_ffi.dll") $ffiExports

    Copy-Item (Join-Path $x64 "SlimeIME.dll") `
        (Join-Path $stagingDirectory "SlimeIME_x64.dll")
    Copy-Item (Join-Path $arm64 "SlimeIME.dll") `
        (Join-Path $stagingDirectory "SlimeIME_arm64.dll")
    Copy-Item (Join-Path $x64 "slime_ffi.dll") `
        (Join-Path $stagingDirectory "slime_ffi_x64.dll")
    Copy-Item (Join-Path $arm64 "slime_ffi.dll") `
        (Join-Path $stagingDirectory "slime_ffi_arm64.dll")
    Copy-Item (Join-Path $arm64 "SlimeIMERegister.exe") $stagingDirectory
    Copy-Item (Join-Path $arm64 "SlimeSettings.exe") $stagingDirectory

    $emptySource = Join-Path $manifestDirectory "empty.cpp"
    $arm64Object = Join-Path $temporaryDirectory "empty-arm64.obj"
    $arm64EcObject = Join-Path $temporaryDirectory "empty-arm64ec.obj"
    $resourceSource = Join-Path $temporaryDirectory "source-revision.rc"
    $resourceObject = Join-Path $temporaryDirectory "source-revision.res"
    Invoke-NativeTool "cl.exe" @(
        "/nologo", "/c", "/Fo$arm64Object", $emptySource
    ) | Out-Null
    Invoke-NativeTool "cl.exe" @(
        "/nologo", "/c", "/arm64EC", "/Fo$arm64EcObject", $emptySource
    ) | Out-Null
    $resourceContents = @"
#include <winver.h>
1 VERSIONINFO
FILEVERSION $($versionParts[0]),$($versionParts[1]),$($versionParts[2]),0
PRODUCTVERSION $($versionParts[0]),$($versionParts[1]),$($versionParts[2]),0
FILEFLAGSMASK VS_FFI_FILEFLAGSMASK
FILEFLAGS 0
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_DLL
FILESUBTYPE VFT2_UNKNOWN
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "Comments", "Source revision: $SourceRevision\0"
      VALUE "FileVersion", "$productVersion\0"
      VALUE "ProductName", "Slime\0"
      VALUE "ProductVersion", "$productVersion\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"@
    [System.IO.File]::WriteAllText($resourceSource, $resourceContents)
    Invoke-NativeTool "rc.exe" @(
        "/nologo", "/fo$resourceObject", $resourceSource
    ) | Out-Null

    Build-Forwarder `
        "SlimeIME" `
        (Join-Path $stagingDirectory "SlimeIME.dll") `
        "SlimeIME_x64" `
        "SlimeIME_arm64" `
        $imeExports `
        $arm64Object `
        $arm64EcObject `
        $resourceObject
    Build-Forwarder `
        "slime_ffi" `
        (Join-Path $stagingDirectory "slime_ffi.dll") `
        "slime_ffi_x64" `
        "slime_ffi_arm64" `
        $ffiExports `
        $arm64Object `
        $arm64EcObject `
        $resourceObject

    & (Join-Path $PSScriptRoot "verify-windows-arm64x-artifact.ps1") `
        -Artifact $stagingDirectory `
        -ExpectedSourceRevision $SourceRevision `
        -ExpectedVersion $productVersion
    Copy-Item (Join-Path $stagingDirectory "*") $outputPath
} finally {
    if (Test-Path -LiteralPath $temporaryDirectory) {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
    }
}

Write-Output "Built unsigned Windows ARM64X development artifact from $SourceRevision`: $outputPath"
