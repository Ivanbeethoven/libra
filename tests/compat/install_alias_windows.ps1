param([string]$RepoRoot)

$ErrorActionPreference = "Stop"

function Fail([string]$Message) {
    throw "install alias Windows smoke: $Message"
}

$installer = Join-Path $RepoRoot "install.ps1"
$source = Join-Path $RepoRoot "target\debug\libra.exe"
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    $source = Join-Path $RepoRoot "target\x86_64-pc-windows-msvc\release\libra.exe"
}
if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    Fail "no built libra.exe found in target\debug or target\x86_64-pc-windows-msvc\release"
}
$sourceOutput = (& $source --version | Out-String).Trim()
$sourceVersionMatch = [regex]::Match($sourceOutput, 'v?[0-9]+\.[0-9]+\.[0-9]+[A-Za-z0-9.+-]*')
if (-not $sourceVersionMatch.Success) {
    Fail "could not determine the version of $source"
}
$sourceVersion = $sourceVersionMatch.Value
if (-not $sourceVersion.StartsWith("v")) {
    $sourceVersion = "v$sourceVersion"
}

$work = Join-Path ([IO.Path]::GetTempPath()) "libra-install-alias-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $work -Force | Out-Null

function Invoke-Installer([string]$InstallDirectory, [string[]]$ExtraArguments) {
    $arguments = @(
        "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $installer,
        "-Version", $sourceVersion, "-Dir", $InstallDirectory, "-BinaryPath", $source,
        "-NoModifyPath"
    ) + $ExtraArguments
    & powershell.exe @arguments
    if ($LASTEXITCODE -ne 0) {
        Fail "installer failed for $InstallDirectory"
    }
}

try {
    $installDir = Join-Path $work "managed"
    Invoke-Installer $installDir @()
    $libraVersion = (& (Join-Path $installDir "libra.exe") --version | Out-String).Trim()
    $lbaVersion = (& (Join-Path $installDir "lba.cmd") --version | Out-String).Trim()
    if ($libraVersion -ne $lbaVersion) {
        Fail "lba --version differs from libra --version"
    }

    $binaryHashBefore = (Get-FileHash -LiteralPath (Join-Path $installDir "libra.exe") -Algorithm SHA256).Hash
    Remove-Item -LiteralPath (Join-Path $installDir "lba.cmd") -Force
    $rerunOutput = Invoke-Installer $installDir @()
    if (($rerunOutput -join "`n") -notmatch "already installed") {
        Fail "same-version install did not take the early-return path"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $installDir "lba.cmd") -PathType Leaf)) {
        Fail "same-version install did not repair lba.cmd"
    }
    $binaryHashAfter = (Get-FileHash -LiteralPath (Join-Path $installDir "libra.exe") -Algorithm SHA256).Hash
    if ($binaryHashBefore -ne $binaryHashAfter) {
        Fail "same-version repair replaced libra.exe"
    }
    Invoke-Installer $installDir @("-NoAlias")
    if (-not (Test-Path -LiteralPath (Join-Path $installDir "lba.cmd") -PathType Leaf)) {
        Fail "-NoAlias removed an existing Libra-managed lba.cmd"
    }

    Invoke-Installer $installDir @("-Uninstall")
    if (Test-Path -LiteralPath (Join-Path $installDir "libra.exe")) {
        Fail "uninstall left Libra-managed libra.exe"
    }
    if (Test-Path -LiteralPath (Join-Path $installDir "lba.cmd")) {
        Fail "uninstall left Libra-managed lba.cmd"
    }

    $foreignDir = Join-Path $work "foreign"
    New-Item -ItemType Directory -Path $foreignDir -Force | Out-Null
    $foreignPath = Join-Path $foreignDir "lba.cmd"
    $foreignContent = "@echo foreign`r`n"
    [IO.File]::WriteAllText($foreignPath, $foreignContent)
    Invoke-Installer $foreignDir @()
    if ([IO.File]::ReadAllText($foreignPath) -ne $foreignContent) {
        Fail "foreign lba.cmd was overwritten"
    }
    Invoke-Installer $foreignDir @("-Uninstall")
    if ([IO.File]::ReadAllText($foreignPath) -ne $foreignContent) {
        Fail "uninstall removed foreign lba.cmd"
    }
    $foreignPs1Path = Join-Path $foreignDir "lba.ps1"
    $foreignPs1Content = "Write-Output 'foreign'`r`n"
    [IO.File]::WriteAllText($foreignPs1Path, $foreignPs1Content)
    Invoke-Installer $foreignDir @()
    if ([IO.File]::ReadAllText($foreignPs1Path) -ne $foreignPs1Content) {
        Fail "foreign lba.ps1 was overwritten"
    }

    $unmanagedDir = Join-Path $work "unmanaged"
    New-Item -ItemType Directory -Path $unmanagedDir -Force | Out-Null
    $unmanagedBinary = Join-Path $unmanagedDir "libra.exe"
    Copy-Item -LiteralPath $source -Destination $unmanagedBinary
    Invoke-Installer $unmanagedDir @()
    if (Test-Path -LiteralPath (Join-Path $unmanagedDir ".libra-install.json")) {
        Fail "same-version external libra.exe was incorrectly adopted"
    }
    Invoke-Installer $unmanagedDir @("-Uninstall")
    if (-not (Test-Path -LiteralPath $unmanagedBinary -PathType Leaf)) {
        Fail "uninstall removed an external libra.exe"
    }

    $noAliasDir = Join-Path $work "no-alias"
    Invoke-Installer $noAliasDir @("-NoAlias")
    if (Test-Path -LiteralPath (Join-Path $noAliasDir "lba.cmd")) {
        Fail "-NoAlias created lba.cmd"
    }

    $envAliasDir = Join-Path $work "env-no-alias"
    $previousNoAlias = $env:LIBRA_NO_ALIAS
    try {
        $env:LIBRA_NO_ALIAS = "1"
        Invoke-Installer $envAliasDir @()
    }
    finally {
        $env:LIBRA_NO_ALIAS = $previousNoAlias
    }
    if (Test-Path -LiteralPath (Join-Path $envAliasDir "lba.cmd")) {
        Fail "LIBRA_NO_ALIAS=1 created lba.cmd"
    }

    Write-Output "install alias Windows smoke: ok"
}
finally {
    if (Test-Path -LiteralPath $work) {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}
