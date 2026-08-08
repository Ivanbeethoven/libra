[CmdletBinding()]
param(
    [string]$Version,
    [string]$Dir,
    [string]$BaseUrl = $env:LIBRA_BASE_URL,
    [string]$BinaryPath,
    [switch]$NoModifyPath,
    [switch]$NoAlias,
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"

function Fail([string]$Message) {
    throw "libra installer: $Message"
}

function Get-UserHome {
    $userProfile = [Environment]::GetEnvironmentVariable("USERPROFILE")
    if ([string]::IsNullOrWhiteSpace($userProfile)) {
        $userProfile = [Environment]::GetFolderPath("UserProfile")
    }
    if ([string]::IsNullOrWhiteSpace($userProfile)) {
        Fail "could not determine the Windows user profile"
    }
    return $userProfile
}

function Get-InstallDirectory {
    param([string]$Requested)

    if (-not [string]::IsNullOrWhiteSpace($Requested)) {
        return [IO.Path]::GetFullPath($Requested)
    }
    if (-not [string]::IsNullOrWhiteSpace($env:LIBRA_INSTALL_DIR)) {
        return [IO.Path]::GetFullPath($env:LIBRA_INSTALL_DIR)
    }

    $libraHome = $env:LIBRA_HOME
    if ([string]::IsNullOrWhiteSpace($libraHome)) {
        $libraHome = Join-Path (Get-UserHome) ".libra"
    }
    return [IO.Path]::GetFullPath((Join-Path $libraHome "bin"))
}

function Normalize-Version([string]$Value) {
    if ([string]::IsNullOrWhiteSpace($Value)) {
        return ""
    }
    if ($Value -notmatch '^v?[0-9]+\.[0-9]+\.[0-9]+[A-Za-z0-9.+-]*$') {
        Fail "invalid version '$Value'"
    }
    if ($Value.StartsWith("v")) {
        return $Value
    }
    return "v$Value"
}

function Get-LatestVersion {
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/libra-tools/libra/releases/latest" -Headers @{ "User-Agent" = "libra-installer" }
        return Normalize-Version ([string]$release.tag_name)
    }
    catch {
        Fail "could not determine the latest version; pass -Version <VERSION> and retry"
    }
}

function Get-LibraVersion([string]$Path) {
    try {
        $output = & $Path --version 2>$null
        if ($LASTEXITCODE -ne 0) {
            return ""
        }
        $text = ($output -join "`n")
        $match = [regex]::Match($text, 'v?[0-9]+\.[0-9]+\.[0-9]+[A-Za-z0-9.+-]*')
        if ($match.Success) {
            return Normalize-Version $match.Value
        }
    }
    catch {
        return ""
    }
    return ""
}

function Get-LbaShimContent {
    return "@echo off`r`nrem Libra-managed lba.cmd; do not edit.`r`n`"%~dp0libra.exe`" %*`r`nexit /b %ERRORLEVEL%`r`n"
}

function Test-LibraAlias([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $false
    }
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        return $false
    }
    $actual = [IO.File]::ReadAllText($Path) -replace "`r`n", "`n"
    $expected = (Get-LbaShimContent) -replace "`r`n", "`n"
    return [string]::Equals($actual, $expected, [StringComparison]::Ordinal)
}

function Write-LbaShim([string]$Path) {
    $temporary = "$Path.$([guid]::NewGuid().ToString('N')).tmp"
    try {
        $utf8 = New-Object Text.UTF8Encoding($false)
        [IO.File]::WriteAllText($temporary, (Get-LbaShimContent), $utf8)
        Move-Item -LiteralPath $temporary -Destination $Path -Force
    }
    finally {
        if (Test-Path -LiteralPath $temporary) {
            Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
        }
    }
}

function Ensure-LbaAlias([string]$InstallDirectory, [bool]$Enabled) {
    if (-not $Enabled) {
        return
    }

    $aliasPath = Join-Path $InstallDirectory "lba.cmd"
    if (Test-Path -LiteralPath $aliasPath) {
        if (Test-LibraAlias $aliasPath) {
            Write-LbaShim $aliasPath
            return
        }
        Write-Warning "lba.cmd already exists and is not a Libra alias; leaving it unchanged"
        return
    }

    Write-LbaShim $aliasPath
}

function Remove-LibraAlias([string]$InstallDirectory) {
    $aliasPath = Join-Path $InstallDirectory "lba.cmd"
    if (Test-LibraAlias $aliasPath) {
        Remove-Item -LiteralPath $aliasPath -Force
    }
}

function Update-UserPath([string]$InstallDirectory) {
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @($current -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $alreadyPresent = @($entries | Where-Object { [StringComparer]::OrdinalIgnoreCase.Equals($_.TrimEnd('\'), $InstallDirectory.TrimEnd('\')) })
    if ($alreadyPresent.Count -eq 0) {
        $entries += $InstallDirectory
        [Environment]::SetEnvironmentVariable("Path", ($entries -join ';'), "User")
        return $true
    }
    return $false
}

function Remove-UserPath([string]$InstallDirectory) {
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @($current -split ';' | Where-Object {
            -not [string]::IsNullOrWhiteSpace($_) -and
            -not [StringComparer]::OrdinalIgnoreCase.Equals($_.TrimEnd('\'), $InstallDirectory.TrimEnd('\'))
        })
    [Environment]::SetEnvironmentVariable("Path", ($entries -join ';'), "User")
}

function Get-InstallMarker([string]$InstallDirectory) {
    $marker = Join-Path $InstallDirectory ".libra-install.json"
    if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) {
        return $null
    }
    try {
        $metadata = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
        if ($metadata.product -eq "libra" -and $metadata.binary -eq "libra.exe") {
            return $metadata
        }
    }
    catch {
        return $null
    }
    return $null
}

function Write-InstallMarker([string]$InstallDirectory, [bool]$PathAdded) {
    $metadata = @{ schema_version = 1; product = "libra"; binary = "libra.exe"; path_added = $PathAdded } | ConvertTo-Json -Compress
    [IO.File]::WriteAllText((Join-Path $InstallDirectory ".libra-install.json"), $metadata, (New-Object Text.UTF8Encoding($false)))
}

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Verify-Checksum([string]$Path, [string]$Url) {
    $sumPath = "$Path.sha256"
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$Url.sha256" -OutFile $sumPath
        $expected = ((Get-Content -LiteralPath $sumPath -Raw) -split '\s+')[0].ToLowerInvariant()
        if ($expected -notmatch '^[0-9a-f]{64}$') {
            Fail "checksum file at $Url.sha256 is malformed"
        }
        $actual = Get-Sha256 $Path
        if ($expected -ne $actual) {
            Fail "sha256 mismatch for $Url (expected $expected, got $actual)"
        }
    }
    catch [System.Net.WebException] {
        if ($env:LIBRA_REQUIRE_CHECKSUM -eq "1") {
            Fail "no checksum published at $Url.sha256; unset LIBRA_REQUIRE_CHECKSUM or wait for a release that publishes one"
        }
        Write-Warning "checksum not published at the mirror; skipping verification"
    }
    finally {
        Remove-Item -LiteralPath $sumPath -Force -ErrorAction SilentlyContinue
    }
}

function Remove-Installation([string]$InstallDirectory) {
    $binary = Join-Path $InstallDirectory "libra.exe"
    $marker = Join-Path $InstallDirectory ".libra-install.json"
    if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) {
        Write-Warning "no Libra install marker found; leaving installed files unchanged"
        return
    }

    try {
        $metadata = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
    }
    catch {
        Write-Warning "could not validate $marker; leaving installed files unchanged"
        return
    }
    if ($metadata.product -ne "libra" -or $metadata.binary -ne "libra.exe") {
        Write-Warning "$marker is not a Libra install marker; leaving installed files unchanged"
        return
    }

    Remove-LibraAlias $InstallDirectory
    if (Test-Path -LiteralPath $binary) {
        Remove-Item -LiteralPath $binary -Force
    }
    Remove-Item -LiteralPath $marker -Force
    if ($metadata.path_added -eq $true) {
        Remove-UserPath $InstallDirectory
    }
}

$installDirectory = Get-InstallDirectory $Dir
$aliasEnabled = -not ($NoAlias -or $env:LIBRA_NO_ALIAS -eq "1")

if ($Uninstall) {
    Remove-Installation $installDirectory
    Write-Output "libra uninstalled from $installDirectory"
    exit 0
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = $env:LIBRA_VERSION
}
if ([string]::IsNullOrWhiteSpace($Version)) {
    $Version = Get-LatestVersion
}
$Version = Normalize-Version $Version

New-Item -ItemType Directory -Path $installDirectory -Force | Out-Null
$target = Join-Path $installDirectory "libra.exe"
$existingMarker = Get-InstallMarker $installDirectory
$pathAdded = $existingMarker -and $existingMarker.path_added -eq $true
$existingVersion = ""
if (Test-Path -LiteralPath $target -PathType Leaf) {
    $existingVersion = Get-LibraVersion $target
}

if ($existingVersion -eq $Version) {
    if ($null -eq $existingMarker) {
        Write-Warning "$target is not managed by the Libra installer; leaving it unchanged"
        exit 0
    }
    Ensure-LbaAlias $installDirectory $aliasEnabled
    if (-not $NoModifyPath) {
        $pathAdded = (Update-UserPath $installDirectory) -or $pathAdded
    }
    Write-InstallMarker $installDirectory $pathAdded
    Write-Output "libra $Version is already installed at $target"
    exit 0
}

$temporary = Join-Path ([IO.Path]::GetTempPath()) "libra-$([guid]::NewGuid().ToString('N')).exe"
try {
    if (-not [string]::IsNullOrWhiteSpace($BinaryPath)) {
        Copy-Item -LiteralPath $BinaryPath -Destination $temporary -Force
    }
    else {
        if ([string]::IsNullOrWhiteSpace($BaseUrl)) {
            $BaseUrl = "https://download.libra.tools/libra/releases"
        }
        $url = "$($BaseUrl.TrimEnd('/'))/$Version/libra-windows-amd64.exe"
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $temporary
        Verify-Checksum $temporary $url
    }
    if ((Get-Item -LiteralPath $temporary).Length -eq 0) {
        Fail "downloaded binary is empty"
    }
    Move-Item -LiteralPath $temporary -Destination $target -Force
}
finally {
    if (Test-Path -LiteralPath $temporary) {
        Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
    }
}

Ensure-LbaAlias $installDirectory $aliasEnabled
if (-not $NoModifyPath) {
    $pathAdded = (Update-UserPath $installDirectory) -or $pathAdded
}
Write-InstallMarker $installDirectory $pathAdded
Write-Output "libra $Version installed at $target"
