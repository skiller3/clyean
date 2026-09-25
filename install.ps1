# Clyean Installer for Windows
# Usage: irm https://clyean.com/install.ps1 | iex
#
# Or with options:
#   & ([scriptblock]::Create((irm https://clyean.com/install.ps1))) -Binary
#   & ([scriptblock]::Create((irm https://clyean.com/install.ps1))) -Source
#   & ([scriptblock]::Create((irm https://clyean.com/install.ps1))) -Binary -Ref v0.2.0
#   & ([scriptblock]::Create((irm https://clyean.com/install.ps1))) -Source -Ref v0.2.0
#   & ([scriptblock]::Create((irm https://clyean.com/install.ps1))) -NoDeps
#   & ([scriptblock]::Create((irm https://clyean.com/install.ps1))) -DryRun
#
# Parameters:
#   -Binary    Install the prebuilt clyean.exe from GitHub releases (default)
#   -Source    Build and install clyean from source with cargo (requires a Rust toolchain)
#   -Ref       Install a specific release tag, for example v0.2.0 (default: latest release)
#   -NoDeps    Do not install missing host dependencies (git, podman); print instructions instead
#   -DryRun    Print the commands the installer would run, including the Podman machine's size, and change nothing
#
# Environment:
#   CLYEAN_INSTALL_DIR   Directory that receives clyean.exe (default: %LOCALAPPDATA%\clyean)
#   CLYEAN_INSTALL_HOST_CPUS, CLYEAN_INSTALL_HOST_MEMORY_MIB
#                        Stand in for the host's logical processors and memory when sizing a Podman machine (for testing)
#   CONTAINERS_MACHINE_PROVIDER
#                        Podman's machine provider; "hyperv" selects Hyper-V instead of the default WSL
#
# Clyean runs every agent inside a Podman container, so Podman 5.0 or later
# is a required host dependency alongside git. Dependencies are installed with
# winget, and every command that changes the host is printed before it runs.
# The installer creates a Podman machine only when none exists and never
# changes an existing one.

param(
    [switch]$Source,
    [switch]$Binary,
    [string]$Ref,
    [switch]$NoDeps,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

# Fail fast on hosts older than Windows PowerShell 5.1: cmdlets used below
# (for example Invoke-WebRequest -TimeoutSec) are missing or unreliable there.
if ($PSVersionTable.PSVersion -lt [version]"5.1") {
    throw "Windows PowerShell 5.1 or newer is required (found $($PSVersionTable.PSVersion)). Install PowerShell 7 from https://aka.ms/powershell and re-run the installer."
}

$Repo = "skiller3/clyean"
$InstallDir = if ($env:CLYEAN_INSTALL_DIR) { $env:CLYEAN_INSTALL_DIR } else { "$env:LOCALAPPDATA\clyean" }
$GitHubApi = "https://api.github.com/repos/$Repo"
$GitHubDownload = "https://github.com/$Repo/releases/download"

# Windows PowerShell 5.1 (.NET Framework) does not reliably resolve
# [System.Runtime.InteropServices.RuntimeInformation] without an
# assembly-qualified name, while PowerShell 7+ loads that type from a
# different assembly, so read the OS architecture from the environment
# instead, which works on both. Prefer PROCESSOR_ARCHITEW6432 so a 32-bit
# host process on 64-bit Windows still reports the OS architecture.
$RawArchitecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if (-not $RawArchitecture) {
    throw "Unable to determine Windows architecture"
}
$NativeArchitecture = switch ($RawArchitecture.ToUpperInvariant()) {
    "AMD64" { "x64" }
    "ARM64" { "arm64" }
    default { throw "Unsupported Windows architecture: $RawArchitecture" }
}
$BinaryName = "clyean-windows-$NativeArchitecture.exe"

# PowerShell 5.1 raises a terminating NativeCommandError for any line a native
# executable writes to stderr while $ErrorActionPreference is "Stop", regardless
# of the process exit code. Tools like winget, cargo, and git emit normal
# progress on stderr, so run them with the preference relaxed to "Continue" and
# gate on $LASTEXITCODE instead.
function Invoke-Native {
    param([Parameter(Mandatory = $true)][scriptblock]$Command)
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $Command
    } finally {
        $ErrorActionPreference = $previous
    }
}

function Test-CommandInstalled {
    param([string]$Name)
    try {
        $null = Get-Command $Name -ErrorAction Stop
        return $true
    } catch {
        return $false
    }
}

# Prints a command that changes the host, then runs it unless this is a dry run.
function Invoke-Logged {
    param([Parameter(Mandatory = $true)][string]$Display, [Parameter(Mandatory = $true)][scriptblock]$Command)
    Write-Host "+ $Display"
    if ($DryRun) {
        $global:LASTEXITCODE = 0
        return
    }
    Invoke-Native $Command
}

function Update-SessionPath {
    $env:Path = [System.Environment]::GetEnvironmentVariable("Path", "User") + ";" + [System.Environment]::GetEnvironmentVariable("Path", "Machine")
}

# ---------------------------------------------------------------------------
# Host dependencies: git, podman
# ---------------------------------------------------------------------------

function Get-MissingDependencies {
    $missing = @()
    if (-not (Test-CommandInstalled "git")) { $missing += "git" }
    if (-not (Test-CommandInstalled "podman")) { $missing += "podman" }
    return $missing
}

function Write-DependencyInstructions {
    param([string[]]$Missing)
    Write-Host ""
    Write-Host "Install the missing dependencies, then re-run this installer:" -ForegroundColor Cyan
    if ($Missing -contains "git") {
        Write-Host "    winget install --id Git.Git -e --source winget" -ForegroundColor Cyan
    }
    if ($Missing -contains "podman") {
        Write-Host "    winget install --id RedHat.Podman -e --source winget" -ForegroundColor Cyan
        Write-Host "    podman machine init" -ForegroundColor Cyan
        Write-Host "    podman machine start" -ForegroundColor Cyan
        Write-Host "Podman on Windows uses WSL 2; see https://podman.io/docs/installation#windows" -ForegroundColor Cyan
    }
}

function Install-WingetPackage {
    param([string]$Id)
    Invoke-Logged "winget install --id $Id -e --source winget --accept-package-agreements --accept-source-agreements" {
        winget install --id $Id -e --source winget --accept-package-agreements --accept-source-agreements
    }
    if ($LASTEXITCODE -ne 0) {
        throw "winget failed to install $Id (exit code $LASTEXITCODE)"
    }
    Update-SessionPath
}

# Stops with upgrade instructions when the installed Podman is older than 5.0,
# the oldest release Clyean supports on Windows. Clyean checks again before
# every launch.
function Test-PodmanVersion {
    if (-not (Test-CommandInstalled "podman")) {
        Write-Host "Podman is not installed yet; it must be 5.0 or later."
        return
    }
    $text = (Invoke-Native { podman --version 2>$null }) | Out-String
    $match = [regex]::Match($text, "(\d+)\.(\d+)")
    if ($match.Success -and [int]$match.Groups[1].Value -ge 5) {
        return
    }
    Write-Host "Clyean needs Podman 5.0 or later, but '$($text.Trim())' is installed." -ForegroundColor Red
    Write-Host "Upgrade it with:  winget upgrade RedHat.Podman" -ForegroundColor Cyan
    throw "Podman is older than 5.0"
}

function Get-HostProcessors {
    if ($env:CLYEAN_INSTALL_HOST_CPUS) { return [int]$env:CLYEAN_INSTALL_HOST_CPUS }
    return [Environment]::ProcessorCount
}

function Get-HostMemoryMib {
    if ($env:CLYEAN_INSTALL_HOST_MEMORY_MIB) { return [int64]$env:CLYEAN_INSTALL_HOST_MEMORY_MIB }
    return [int64]((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1MB)
}

# The size WSL gives its virtual machine, which hosts every WSL distribution
# including Podman's: its global `processors` and `memory` settings, which
# default to every logical processor and half of the host's memory.
function Get-WslMachineSize {
    param([bool]$Created)
    if ($Created -and -not $DryRun) {
        $reported = (Invoke-Native { podman info --format "{{.Host.CPUs}} {{.Host.MemTotal}}" 2>$null }) | Out-String
        $fields = $reported.Trim() -split "\s+"
        if ($fields.Count -eq 2) {
            return @{ Cpus = [int]$fields[0]; MemoryMib = [int64]([int64]$fields[1] / 1MB) }
        }
    }
    return @{ Cpus = (Get-HostProcessors); MemoryMib = [int64]((Get-HostMemoryMib) / 2) }
}

# Podman on Windows runs containers inside a Linux virtual machine that must be
# created once and started before Clyean can launch agents. With the default
# WSL provider the machine's CPUs and memory are WSL's global settings, which
# the installer never edits; with Hyper-V the machine is sized like on macOS.
function Ensure-PodmanMachine {
    $machines = $null
    if (Test-CommandInstalled "podman") {
        $machines = Invoke-Native { podman machine list --format "{{.Name}}" 2>$null }
    }
    $created = $false
    if (-not $machines) {
        if ($env:CONTAINERS_MACHINE_PROVIDER -eq "hyperv") {
            $cpus = [Math]::Min(4, (Get-HostProcessors))
            $memory = [Math]::Min([int64]8192, [int64]((Get-HostMemoryMib) / 2))
            Write-Host "[NOTE] Clyean supports the Hyper-V provider on a best-effort basis; the default WSL provider is fully supported." -ForegroundColor Yellow
            Invoke-Logged "podman machine init --cpus $cpus --memory $memory --disk-size 100" {
                podman machine init --cpus $cpus --memory $memory --disk-size 100
            }
        } else {
            Invoke-Logged "podman machine init" { podman machine init }
        }
        if ($LASTEXITCODE -ne 0) {
            Write-Host "[WARN] 'podman machine init' failed. Enable WSL 2 with 'wsl --install', reboot, then run 'podman machine init' and 'podman machine start'." -ForegroundColor Yellow
            return
        }
        $created = $true
    }
    $state = if ($DryRun -and $created) { "" } else { Invoke-Native { podman machine inspect --format "{{.State}}" 2>$null } }
    if ($state -notmatch "running") {
        Invoke-Logged "podman machine start" { podman machine start }
        if ($LASTEXITCODE -ne 0) {
            Write-Host "[WARN] Could not start the Podman machine; run 'podman machine start' before using clyean." -ForegroundColor Yellow
        }
    }
    if ($env:CONTAINERS_MACHINE_PROVIDER -ne "hyperv") {
        $size = Get-WslMachineSize -Created $created
        Write-Host "The Podman machine has $($size.Cpus) CPUs and $([Math]::Round($size.MemoryMib / 1024, 1)) GiB of memory."
        if ($size.Cpus -lt 4 -or $size.MemoryMib -lt 8192) {
            Write-Host "[WARN] Clyean recommends at least 4 CPUs and 8 GiB. WSL sets these for every distribution through 'processors' and 'memory' in the [wsl2] section of $env:UserProfile\.wslconfig; raise them there, then run 'wsl --shutdown' and 'podman machine start'." -ForegroundColor Yellow
        }
    }
}

function Ensure-Dependencies {
    $missing = Get-MissingDependencies
    if ($missing.Count -gt 0) {
        if ($NoDeps) {
            Write-Host "Missing dependencies: $($missing -join ', ') (-NoDeps given, not installing)." -ForegroundColor Yellow
            Write-DependencyInstructions $missing
            throw "Missing dependencies: $($missing -join ', ')"
        }
        if (-not (Test-CommandInstalled "winget") -and -not $DryRun) {
            Write-Host "winget is required to install dependencies. Install 'App Installer' from the Microsoft Store." -ForegroundColor Yellow
            Write-DependencyInstructions $missing
            throw "winget not found"
        }
        Write-Host "Installing missing dependencies: $($missing -join ', ')"
        if ($missing -contains "git") { Install-WingetPackage "Git.Git" }
        if ($missing -contains "podman") { Install-WingetPackage "RedHat.Podman" }
        $stillMissing = Get-MissingDependencies
        if ($stillMissing.Count -gt 0 -and -not $DryRun) {
            Write-DependencyInstructions $stillMissing
            throw "Dependencies still missing after installation: $($stillMissing -join ', '). Restart the terminal and re-run the installer."
        }
    }
    Test-PodmanVersion
    if (-not $NoDeps) {
        Ensure-PodmanMachine
    }
}

# ---------------------------------------------------------------------------
# Release lookup
# ---------------------------------------------------------------------------

function Get-ReleaseTag {
    if ($Ref) {
        Write-Host "Fetching release $Ref..."
        try {
            $release = Invoke-RestMethod -Uri "$GitHubApi/releases/tags/$Ref" -TimeoutSec 60
        } catch {
            throw "Release tag not found: $Ref"
        }
    } else {
        Write-Host "Fetching latest release..."
        $release = Invoke-RestMethod -Uri "$GitHubApi/releases/latest" -TimeoutSec 60
    }
    $tag = $release.tag_name
    if (-not $tag) {
        throw "Failed to fetch release tag"
    }
    return $tag
}

# ---------------------------------------------------------------------------
# Source install (cargo)
# ---------------------------------------------------------------------------

function Install-ViaCargo {
    if (-not (Test-CommandInstalled "cargo")) {
        throw "cargo is required for -Source. Install a Rust toolchain from https://rustup.rs and re-run."
    }
    $tag = Get-ReleaseTag
    Write-Host "Building clyean $tag from source..."
    $tmpRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("clyean-install-" + [System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tmpRoot | Out-Null
    try {
        Write-Host "+ cargo install --locked --git https://github.com/$Repo --tag $tag --root $tmpRoot clyean"
        Invoke-Native { cargo install --locked --git "https://github.com/$Repo" --tag $tag --root $tmpRoot clyean }
        if ($LASTEXITCODE -ne 0) {
            throw "cargo install failed (exit code $LASTEXITCODE)"
        }
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        Copy-Item -Path (Join-Path $tmpRoot "bin\clyean.exe") -Destination (Join-Path $InstallDir "clyean.exe") -Force
    } finally {
        Remove-Item -Recurse -Force $tmpRoot -ErrorAction SilentlyContinue
    }
    Complete-Install
}

# ---------------------------------------------------------------------------
# Binary install (GitHub releases)
# ---------------------------------------------------------------------------

function Test-Checksum {
    param([string]$File, [string]$Asset, [string]$SumsFile)
    $expected = $null
    foreach ($line in Get-Content $SumsFile) {
        $parts = $line.Trim() -split "\s+", 2
        if ($parts.Count -eq 2 -and ($parts[1] -eq $Asset -or $parts[1] -eq "*$Asset")) {
            $expected = $parts[0].ToLowerInvariant()
            break
        }
    }
    if (-not $expected) {
        throw "SHA256SUMS for this release has no entry for $Asset."
    }
    $actual = (Get-FileHash -Path $File -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "Checksum mismatch for ${Asset}: expected $expected, actual $actual"
    }
    Write-Host "Checksum verified."
}

function Install-Binary {
    $tag = Get-ReleaseTag
    Write-Host "Using version: $tag"

    $tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("clyean-install-" + [System.Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null
    try {
        $binaryPath = Join-Path $tmpDir $BinaryName
        $sumsPath = Join-Path $tmpDir "SHA256SUMS"
        Write-Host "Downloading $BinaryName..."
        Invoke-WebRequest -Uri "$GitHubDownload/$tag/$BinaryName" -OutFile $binaryPath -TimeoutSec 900
        Invoke-WebRequest -Uri "$GitHubDownload/$tag/SHA256SUMS" -OutFile $sumsPath -TimeoutSec 60
        Test-Checksum -File $binaryPath -Asset $BinaryName -SumsFile $sumsPath

        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        Move-Item -Path $binaryPath -Destination (Join-Path $InstallDir "clyean.exe") -Force
    } finally {
        Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
    }
    Complete-Install
}

# Verify the freshly installed binary can actually start before reporting
# success, then make sure the install directory is on the user PATH.
function Complete-Install {
    $exePath = Join-Path $InstallDir "clyean.exe"
    $smokeOutput = Invoke-Native { & $exePath --version 2>&1 }
    if ($LASTEXITCODE -ne 0) {
        Write-Host ""
        Write-Host "[FAIL] clyean was installed to $exePath but cannot start:" -ForegroundColor Red
        Write-Host ($smokeOutput | Out-String)
        throw "clyean --version failed"
    }

    Write-Host ""
    Write-Host "[OK] Installed $smokeOutput to $exePath" -ForegroundColor Green

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $needsRestart = $userPath -notlike "*$InstallDir*"
    if ($needsRestart) {
        Write-Host "Adding $InstallDir to PATH..."
        [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
    }

    if ($needsRestart) {
        Write-Host "Restart your terminal, then run 'clyean' inside a project directory to get started!"
    } else {
        Write-Host "Run 'clyean' inside a project directory to get started!"
    }
}

# Main logic
if ($Source -and $Binary) {
    throw "Choose either -Source or -Binary, not both."
}

Ensure-Dependencies

if ($DryRun) {
    $mode = if ($Source) { "source" } else { "binary" }
    $version = if ($Ref) { $Ref } else { "(latest release)" }
    Write-Host "Dry run: would install the $mode build of clyean $version for windows-$NativeArchitecture into $InstallDir."
    return
}

if ($Source) {
    Install-ViaCargo
} else {
    Install-Binary
}
