function Get-SlimeNativePayloadSpecification([string]$NativeDirectory = "x64") {
    if ($NativeDirectory -eq "arm64x") {
        return @(
            [PSCustomObject]@{ Name = "SlimeIME.dll"; Machine = [UInt16]0 },
            [PSCustomObject]@{ Name = "SlimeIME_arm64.dll"; Machine = [UInt16]0xAA64 },
            [PSCustomObject]@{ Name = "SlimeIME_x64.dll"; Machine = [UInt16]0x8664 },
            [PSCustomObject]@{ Name = "SlimeIMERegister.exe"; Machine = [UInt16]0xAA64 },
            [PSCustomObject]@{ Name = "SlimeSettings.exe"; Machine = [UInt16]0xAA64 },
            [PSCustomObject]@{ Name = "slime_ffi.dll"; Machine = [UInt16]0 },
            [PSCustomObject]@{ Name = "slime_ffi_arm64.dll"; Machine = [UInt16]0xAA64 },
            [PSCustomObject]@{ Name = "slime_ffi_x64.dll"; Machine = [UInt16]0x8664 }
        )
    }
    if ($NativeDirectory -ne "x64") {
        throw "Unsupported native installer directory: $NativeDirectory"
    }
    return @(
        [PSCustomObject]@{ Name = "SlimeIME.dll"; Machine = [UInt16]0x8664 },
        [PSCustomObject]@{ Name = "SlimeIMERegister.exe"; Machine = [UInt16]0x8664 },
        [PSCustomObject]@{ Name = "SlimeSettings.exe"; Machine = [UInt16]0x8664 },
        [PSCustomObject]@{ Name = "slime_ffi.dll"; Machine = [UInt16]0x8664 }
    )
}

function Get-SlimeNativePayloadFiles([string]$NativeDirectory = "x64") {
    return @(
        Get-SlimeNativePayloadSpecification $NativeDirectory |
            ForEach-Object { $_.Name } |
            Sort-Object
    )
}

function Get-SlimeExpectedInstalledFiles([string]$NativeDirectory = "x64") {
    $files = @("Uninstall.exe")
    $nativeFiles = Get-SlimeNativePayloadFiles $NativeDirectory
    foreach ($file in $nativeFiles) {
        $files += "$NativeDirectory\$file"
    }
    $x86Files = Get-SlimeNativePayloadFiles "x64"
    foreach ($file in $x86Files) {
        $files += "x86\$file"
    }
    return @($files | Sort-Object)
}

function Assert-SlimeInstalledLayout(
    [string]$InstallDirectory,
    [string]$NativeDirectory = "x64"
) {
    $root = Get-Item -LiteralPath $InstallDirectory -Force
    $items = @(Get-ChildItem -LiteralPath $root.FullName -Force -Recurse)
    $reparsePoints = @(
        @($root) + $items | Where-Object {
            ($_.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
        }
    )
    if ($reparsePoints.Count -ne 0) {
        throw "Installed payload contains a reparse point: $($reparsePoints[0].FullName)"
    }

    $separator = [System.IO.Path]::DirectorySeparatorChar
    $prefix = $root.FullName.TrimEnd($separator) + $separator
    $actualFiles = @(
        $items |
            Where-Object { -not $_.PSIsContainer } |
            ForEach-Object {
                $_.FullName.Substring($prefix.Length).Replace($separator, "\")
            } |
            Sort-Object
    )
    $expectedFiles = Get-SlimeExpectedInstalledFiles $NativeDirectory
    $fileDifference = @(Compare-Object $expectedFiles $actualFiles)
    if ($fileDifference.Count -ne 0) {
        throw "Installed payload file list is unexpected: $($fileDifference | Out-String)"
    }

    $actualDirectories = @(
        $items |
            Where-Object { $_.PSIsContainer } |
            ForEach-Object {
                $_.FullName.Substring($prefix.Length).Replace($separator, "\")
            } |
            Sort-Object
    )
    $expectedDirectories = @($NativeDirectory, "x86") | Sort-Object
    $directoryDifference = @(Compare-Object $expectedDirectories $actualDirectories)
    if ($directoryDifference.Count -ne 0) {
        throw "Installed payload directory list is unexpected: $($directoryDifference | Out-String)"
    }
}

function Get-SlimeInstalledManifestSha256(
    [string]$InstallDirectory,
    [string]$NativeDirectory = "x64"
) {
    Assert-SlimeInstalledLayout $InstallDirectory $NativeDirectory
    $lines = @()
    foreach ($relativePath in Get-SlimeExpectedInstalledFiles $NativeDirectory) {
        $platformPath = $relativePath.Replace(
            "\",
            [System.IO.Path]::DirectorySeparatorChar
        )
        $path = Join-Path $InstallDirectory $platformPath
        $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash
        $lines += "$relativePath`t$hash"
    }
    $canonical = ($lines -join "`n") + "`n"
    $utf8WithoutBom = New-Object System.Text.UTF8Encoding($false)
    $hasher = [Security.Cryptography.SHA256]::Create()
    try {
        $bytes = $utf8WithoutBom.GetBytes($canonical)
        return ([BitConverter]::ToString($hasher.ComputeHash($bytes))).Replace("-", "")
    } finally {
        $hasher.Dispose()
    }
}

function Assert-SlimeUninstallRegistration(
    [object]$Properties,
    [string]$ExpectedVersion,
    [string]$InstallDirectory,
    [string]$NativeDirectory = "x64",
    [string]$ExpectedSourceRevision = "development"
) {
    $settings = Join-Path $InstallDirectory "$NativeDirectory\SlimeSettings.exe"
    $uninstaller = Join-Path $InstallDirectory "Uninstall.exe"
    $expectedUninstall = "`"$uninstaller`""
    if ($Properties.DisplayName -ne "Slime Japanese IME" -or
        $Properties.DisplayVersion -ne $ExpectedVersion -or
        $Properties.SourceRevision -ne $ExpectedSourceRevision -or
        $Properties.Publisher -ne "unvalley" -or
        $Properties.DisplayIcon -ne $settings -or
        $Properties.UninstallString -ne $expectedUninstall -or
        $Properties.QuietUninstallString -ne "$expectedUninstall /S" -or
        $Properties.NoModify -ne 1 -or
        $Properties.NoRepair -ne 1) {
        throw "Uninstall registration values do not match the installed release"
    }
}

function Get-SlimeStartMenuDirectory {
    return Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\Slime"
}

function Assert-SlimeInstallDirectoriesAbsent([string]$ProgramFilesDirectory) {
    $productDirectory = Join-Path $ProgramFilesDirectory "Slime"
    if (Test-Path -LiteralPath $productDirectory) {
        throw "Installed payload directory remains without an active release: $productDirectory"
    }
}

function Assert-SlimeStartMenu(
    [string]$InstallDirectory,
    [string]$NativeDirectory = "x64"
) {
    $directory = Get-SlimeStartMenuDirectory
    if (-not (Test-Path -LiteralPath $directory -PathType Container)) {
        throw "Start Menu directory is missing"
    }
    # Windows PowerShell 5.1 treats a UTF-8 script without a BOM as ANSI.
    # Construct localized shortcut names from code points so this release
    # verifier behaves identically in Windows PowerShell and PowerShell 7.
    $settingsName = "Slime " + [char]0x8A2D + [char]0x5B9A + ".lnk"
    $uninstallName = "Slime " + [char]0x306E + [char]0x30A2 + [char]0x30F3 +
        [char]0x30A4 + [char]0x30F3 + [char]0x30B9 + [char]0x30C8 +
        [char]0x30FC + [char]0x30EB + ".lnk"
    $expected = @{
        $settingsName = Join-Path $InstallDirectory "$NativeDirectory\SlimeSettings.exe"
        $uninstallName = Join-Path $InstallDirectory "Uninstall.exe"
    }
    $actualNames = @(
        Get-ChildItem -LiteralPath $directory -Force |
            ForEach-Object { $_.Name } |
            Sort-Object
    )
    $nameDifference = @(Compare-Object @($expected.Keys | Sort-Object) $actualNames)
    if ($nameDifference.Count -ne 0) {
        throw "Start Menu entries are unexpected: $($nameDifference | Out-String)"
    }

    # WScript.Shell opens shortcuts through the ANSI code page, so a Japanese
    # name yields an empty target on a system with another code page.
    $shell = New-Object -ComObject Shell.Application
    try {
        $folder = $shell.Namespace($directory)
        foreach ($name in $expected.Keys) {
            $item = if ($folder) { $folder.ParseName($name) }
            $target = if ($item -and $item.IsLink) { $item.GetLink.Path }
            if (-not $target -or
                [System.IO.Path]::GetFullPath($target) -ne
                [System.IO.Path]::GetFullPath($expected[$name])) {
                throw "Start Menu shortcut '$name' points to an unexpected target: '$target'"
            }
        }
    } finally {
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell)
    }
}

function Assert-SlimeStartMenuRemoved {
    $directory = Get-SlimeStartMenuDirectory
    if (Test-Path -LiteralPath $directory) {
        throw "Start Menu entries remain after uninstall: $directory"
    }
}

function Assert-SlimeSettingsSelfTest([string]$SettingsExecutable) {
    if (-not (Test-Path -LiteralPath $SettingsExecutable -PathType Leaf)) {
        throw "Settings executable is missing: $SettingsExecutable"
    }
    $temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) `
        "slime-settings-self-test-$([guid]::NewGuid())"
    $originalLocalAppData = [Environment]::GetEnvironmentVariable(
        "LOCALAPPDATA",
        [EnvironmentVariableTarget]::Process
    )
    try {
        New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
        [Environment]::SetEnvironmentVariable(
            "LOCALAPPDATA",
            $temporaryDirectory,
            [EnvironmentVariableTarget]::Process
        )
        $process = Start-Process `
            -FilePath $SettingsExecutable `
            -ArgumentList "--self-test" `
            -PassThru
        if (-not $process.WaitForExit(15000)) {
            $process.Kill()
            $process.WaitForExit()
            throw "Settings self-test did not finish within 15 seconds: $SettingsExecutable"
        }
        if ($process.ExitCode -ne 0) {
            throw "Settings self-test failed with exit code $($process.ExitCode): $SettingsExecutable"
        }
        $remaining = @(Get-ChildItem -LiteralPath $temporaryDirectory -Force -Recurse)
        if ($remaining.Count -ne 0) {
            throw "Settings self-test left user data behind: $($remaining[0].FullName)"
        }
    } finally {
        [Environment]::SetEnvironmentVariable(
            "LOCALAPPDATA",
            $originalLocalAppData,
            [EnvironmentVariableTarget]::Process
        )
        if (Test-Path -LiteralPath $temporaryDirectory) {
            Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
        }
    }
}
