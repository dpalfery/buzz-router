# buzz-router installer for Windows (PowerShell 5.1 or later).
#
#   irm https://raw.githubusercontent.com/dpalfery/buzz-router/main/scripts/install.ps1 | iex
#
# Downloads buzz-router.exe from a GitHub Release and verifies it against the
# release's SHA256SUMS.txt before installing. It installs the binary only: it
# writes no config and does not install the service (see docs/guides/install.md).
#
# Options, as environment variables (a piped script cannot take parameters):
#   $env:BUZZ_ROUTER_VERSION      release to install, e.g. 0.1.0 or v0.1.0-rc.2 (default: latest stable)
#   $env:BUZZ_ROUTER_PRERELEASE   set to 1 to take the newest release, release candidates included
#   $env:BUZZ_ROUTER_INSTALL_DIR  where to put the binary (default: %LOCALAPPDATA%\buzz-router\bin)
#
# Example, pinned to a version:
#   $env:BUZZ_ROUTER_VERSION = '0.1.0-rc.1'; irm https://raw.githubusercontent.com/dpalfery/buzz-router/main/scripts/install.ps1 | iex
#
# To pin the installer itself to a release, fetch it from that release instead of from main:
#   irm https://github.com/dpalfery/buzz-router/releases/download/v0.1.0/install.ps1 | iex

# No param() block: "irm | iex" cannot pass parameters, so options are environment variables only.
$Version = $env:BUZZ_ROUTER_VERSION
$Prerelease = $env:BUZZ_ROUTER_PRERELEASE -eq '1'
$InstallDir = $env:BUZZ_ROUTER_INSTALL_DIR

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Owner = 'dpalfery'
$Repo = 'buzz-router'
# Points at a directory laid out like a GitHub release download area. It exists so the
# installer can be tested against a local fake release; it must be https:// or a file path.
$ReleaseBase = if ($env:BUZZ_ROUTER_RELEASE_BASE) { $env:BUZZ_ROUTER_RELEASE_BASE } else { "https://github.com/$Owner/$Repo/releases/download" }
$LatestUrl = "https://github.com/$Owner/$Repo/releases/latest"
$ReleasesApi = "https://api.github.com/repos/$Owner/$Repo/releases?per_page=30"

function Fail([string]$Message) {
    throw "buzz-router: error: $Message"
}

function Write-Log([string]$Message) {
    Write-Host "buzz-router: $Message"
}

if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA 'buzz-router\bin' }

# Windows PowerShell 5.1 defaults to TLS 1.0/1.1 on some machines.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$isRemote = $ReleaseBase -match '^https://'
if (-not $isRemote -and -not (Test-Path -LiteralPath $ReleaseBase)) {
    Fail 'BUZZ_ROUTER_RELEASE_BASE must start with https:// or be an existing directory'
}

function Get-Asset([string]$Tag, [string]$Name, [string]$Destination) {
    if ($isRemote) {
        Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseBase/$Tag/$Name" -OutFile $Destination
    }
    else {
        Copy-Item -LiteralPath (Join-Path (Join-Path $ReleaseBase $Tag) $Name) -Destination $Destination
    }
}

# Only 64-bit Intel/AMD Windows is built.
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($arch -ne 'AMD64') {
    Fail "unsupported architecture: $arch (only x86_64 Windows is built)"
}
$Target = 'x86_64-pc-windows-msvc'

if ($Version) {
    $Version = $Version.TrimStart('v')
}
else {
    try {
        if ($Prerelease) {
            $releases = Invoke-RestMethod -UseBasicParsing -Uri $ReleasesApi
            $tag = ($releases | Select-Object -First 1).tag_name
        }
        else {
            # /releases/latest redirects to /releases/tag/<tag> and skips pre-releases.
            $response = Invoke-WebRequest -UseBasicParsing -Uri $LatestUrl -Method Head
            $tag = ([string]$response.BaseResponse.ResponseUri.AbsoluteUri -split '/tag/')[-1]
        }
    }
    catch {
        Fail "could not look up the latest release: $($_.Exception.Message)"
    }
    if ($tag -notmatch '^v\d') {
        Fail 'could not find a release (is there one yet? set $env:BUZZ_ROUTER_VERSION, or $env:BUZZ_ROUTER_PRERELEASE=1)'
    }
    $Version = $tag.TrimStart('v')
}

$Tag = "v$Version"
$Archive = "buzz-router-$Version-$Target.zip"
$Work = Join-Path ([IO.Path]::GetTempPath()) ("buzz-router-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $Work | Out-Null

try {
    Write-Log "installing $Version for $Target"
    try {
        Get-Asset $Tag $Archive (Join-Path $Work $Archive)
    }
    catch {
        Fail "cannot download $Archive from release $Tag (does that release exist?)"
    }
    try {
        Get-Asset $Tag 'SHA256SUMS.txt' (Join-Path $Work 'SHA256SUMS.txt')
    }
    catch {
        Fail "cannot download SHA256SUMS.txt from release $Tag"
    }

    # Lines are "<hash>  <file>". Exact filename match, never a prefix.
    $expected = $null
    foreach ($line in Get-Content -LiteralPath (Join-Path $Work 'SHA256SUMS.txt')) {
        $parts = $line.Trim() -split '\s+', 2
        if ($parts.Count -eq 2 -and $parts[1].TrimStart('*') -eq $Archive) {
            $expected = $parts[0].ToLowerInvariant()
            break
        }
    }
    if (-not $expected) { Fail "$Archive is not listed in SHA256SUMS.txt" }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $Work $Archive)).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        Fail "checksum mismatch for $Archive (expected $expected, got $actual); nothing was installed"
    }
    Write-Log 'checksum ok'

    $extract = Join-Path $Work 'x'
    Expand-Archive -LiteralPath (Join-Path $Work $Archive) -DestinationPath $extract
    $exe = Join-Path $extract 'buzz-router.exe'
    if (-not (Test-Path -LiteralPath $exe)) { Fail 'the archive does not contain buzz-router.exe' }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $dest = Join-Path $InstallDir 'buzz-router.exe'
    $previous = $null
    if (Test-Path -LiteralPath $dest) {
        try { $previous = (& $dest --version 2>$null) } catch { $previous = $null }
    }
    # A running exe cannot be overwritten, but it can be renamed away. That lets an upgrade
    # proceed while the service is up; the old copy is removed on a best-effort basis.
    $old = "$dest.old"
    if (Test-Path -LiteralPath $dest) {
        if (Test-Path -LiteralPath $old) { Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue }
        Move-Item -LiteralPath $dest -Destination $old -Force
    }
    Copy-Item -LiteralPath $exe -Destination $dest -Force
    Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue

    $installed = (& $dest --version 2>&1)
    if ($LASTEXITCODE -ne 0) { Fail "installed to $dest but it does not run: $installed" }
    if ($previous -and $previous -ne $installed) {
        Write-Log "upgraded: $previous -> $installed"
    }
    else {
        Write-Log "installed: $installed at $dest"
    }
    Write-Log 'If Windows SmartScreen warns about the first run: the binary is unsigned. The checksum above is the check.'

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @()
    if ($userPath) { $entries = $userPath -split ';' | Where-Object { $_ } }
    if ($entries -notcontains $InstallDir) {
        [Environment]::SetEnvironmentVariable('Path', (($entries + $InstallDir) -join ';'), 'User')
        Write-Log "added $InstallDir to your user PATH. Open a new terminal to pick it up."
    }

    Write-Host ''
    Write-Host 'Next: write router.toml and roster.toml, store each bot''s key, then install the service.'
    Write-Host "  Getting started: https://github.com/$Owner/$Repo/blob/main/docs/guides/getting-started.md"
    Write-Host "  Install notes:   https://github.com/$Owner/$Repo/blob/main/docs/guides/install.md"
}
finally {
    Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
}
