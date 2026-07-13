[CmdletBinding()]
param(
    [ValidateSet("All", "GameBoy", "ZxSpectrum", "Cpm", "Ez180N")]
    [string]$Suite = "All",

    [string]$Ez180NCore = $env:PLAY96_EZ180N_CORE,

    [switch]$Refresh
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($architecture -ne [System.Runtime.InteropServices.Architecture]::X64) {
    throw "The core downloader currently supports x86_64 hosts; detected $architecture. Set the PLAY96_*_CORE variables and run cargo test directly on another architecture."
}

if ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)) {
    $platformPath = "windows/x86_64"
    $libraryExtension = "dll"
    $cachePlatform = "windows-x86_64"
    $ez180NAsset = "ez180n_windows_x64.dll"
}
elseif ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Linux)) {
    $platformPath = "linux/x86_64"
    $libraryExtension = "so"
    $cachePlatform = "linux-x86_64"
    $ez180NAsset = "ez180n_linux_x64.so"
}
elseif ([System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::OSX)) {
    $platformPath = "apple/osx/x86_64"
    $libraryExtension = "dylib"
    $cachePlatform = "macos-x86_64"
    $ez180NAsset = "ez180n_macos_x64.dylib"
}
else {
    throw "Unsupported operating system. Set the PLAY96_*_CORE variables and run cargo test directly."
}

$coreDirectory = Join-Path $repoRoot "target/play96-cores/$cachePlatform"
New-Item -ItemType Directory -Force -Path $coreDirectory | Out-Null

function Get-LibretroCore {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $libraryName = "$Name.$libraryExtension"
    $libraryPath = Join-Path $coreDirectory $libraryName
    if ((Test-Path -LiteralPath $libraryPath) -and -not $Refresh) {
        Write-Host "Using cached RetroArch core: $libraryPath"
        return (Resolve-Path -LiteralPath $libraryPath).Path
    }

    $archiveName = "$libraryName.zip"
    $archivePath = Join-Path $coreDirectory $archiveName
    $uri = "https://buildbot.libretro.com/nightly/$platformPath/latest/$archiveName"
    Write-Host "Downloading RetroArch core: $uri"
    Invoke-WebRequest -Uri $uri -OutFile $archivePath
    Expand-Archive -LiteralPath $archivePath -DestinationPath $coreDirectory -Force
    Remove-Item -LiteralPath $archivePath -Force

    if (-not (Test-Path -LiteralPath $libraryPath -PathType Leaf)) {
        throw "The RetroArch archive did not contain $libraryName"
    }
    return (Resolve-Path -LiteralPath $libraryPath).Path
}

function Get-Sha256Hex {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $hash = $sha256.ComputeHash($stream)
        return [System.BitConverter]::ToString($hash).Replace("-", "").ToLowerInvariant()
    }
    finally {
        $stream.Dispose()
        $sha256.Dispose()
    }
}

function Get-Ez180NCore {
    if (-not [string]::IsNullOrWhiteSpace($Ez180NCore)) {
        if (-not (Test-Path -LiteralPath $Ez180NCore -PathType Leaf)) {
            throw "The ez180N core does not exist: $Ez180NCore"
        }
        return (Resolve-Path -LiteralPath $Ez180NCore).Path
    }

    $libraryPath = Join-Path $coreDirectory $ez180NAsset
    $verifiedHashPath = "$libraryPath.sha256"
    if ((Test-Path -LiteralPath $libraryPath -PathType Leaf) -and
        (Test-Path -LiteralPath $verifiedHashPath -PathType Leaf) -and
        -not $Refresh) {
        $expectedHash = (Get-Content -LiteralPath $verifiedHashPath -Raw).Trim().ToLowerInvariant()
        $actualHash = Get-Sha256Hex -Path $libraryPath
        if ($actualHash -eq $expectedHash) {
            Write-Host "Using cached verified ez180N nightly core: $libraryPath"
            return (Resolve-Path -LiteralPath $libraryPath).Path
        }
        Write-Host "Discarding cached ez180N core with a mismatched checksum"
        Remove-Item -LiteralPath $libraryPath, $verifiedHashPath -Force
    }

    $releaseRoot = "https://codeberg.org/josemancharo/ez180N/releases/download/nightly"
    $checksumsPath = Join-Path $coreDirectory "ez180n-SHA256SUMS"
    Write-Host "Downloading ez180N nightly core: $releaseRoot/$ez180NAsset"
    Invoke-WebRequest -Uri "$releaseRoot/$ez180NAsset" -OutFile $libraryPath
    Invoke-WebRequest -Uri "$releaseRoot/SHA256SUMS" -OutFile $checksumsPath

    $checksumLine = Get-Content -LiteralPath $checksumsPath |
        Where-Object { $_ -match [regex]::Escape($ez180NAsset) } |
        Select-Object -First 1
    Remove-Item -LiteralPath $checksumsPath -Force
    if ([string]::IsNullOrWhiteSpace($checksumLine)) {
        Remove-Item -LiteralPath $libraryPath -Force
        throw "SHA256SUMS does not contain $ez180NAsset"
    }
    $expectedHash = ($checksumLine -split "\s+")[0].ToLowerInvariant()
    $actualHash = Get-Sha256Hex -Path $libraryPath
    if ($actualHash -ne $expectedHash) {
        Remove-Item -LiteralPath $libraryPath -Force
        throw "ez180N checksum mismatch: expected $expectedHash, got $actualHash"
    }
    Set-Content -LiteralPath $verifiedHashPath -Value $expectedHash -NoNewline

    return (Resolve-Path -LiteralPath $libraryPath).Path
}

function Invoke-CoreTest {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    Write-Host "Running $Name"
    Push-Location $repoRoot
    try {
        & cargo test --test libretro_examples $Name -- --ignored --exact --nocapture
        if ($LASTEXITCODE -ne 0) {
            throw "Real-core integration test failed: $Name"
        }
    }
    finally {
        Pop-Location
    }
}

if ($Suite -eq "All" -or $Suite -eq "GameBoy") {
    $env:PLAY96_GAMEBOY_CORE = Get-LibretroCore -Name "mgba_libretro"
    Invoke-CoreTest -Name "gameboy_examples_run_on_real_core"
}

if ($Suite -eq "All" -or $Suite -eq "ZxSpectrum") {
    $env:PLAY96_ZX_SPECTRUM_CORE = Get-LibretroCore -Name "fuse_libretro"
    Invoke-CoreTest -Name "zx_spectrum_example_runs_on_real_core"
}

if ($Suite -eq "All" -or $Suite -eq "Cpm") {
    $env:PLAY96_CPM_CORE = Get-LibretroCore -Name "ep128emu_core_libretro"
    Invoke-CoreTest -Name "cpm_examples_run_on_real_core"
}

if ($Suite -eq "All" -or $Suite -eq "Ez180N") {
    $env:PLAY96_EZ180N_CORE = Get-Ez180NCore
    Invoke-CoreTest -Name "ez180n_examples_run_on_real_core"
}
