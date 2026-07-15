[CmdletBinding()]
param(
    [ValidateSet("All", "GameBoy", "ZxSpectrum", "Cpm", "Ez180N", "Arduboy")]
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
$resultsDirectory = Join-Path $repoRoot "target/play96-results"
$results = @()
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

function Write-TestResults {
    New-Item -ItemType Directory -Force -Path $resultsDirectory | Out-Null
    $generatedAt = [DateTimeOffset]::UtcNow.ToString("o")
    $report = [ordered]@{
        schemaVersion = 1
        generatedAt = $generatedAt
        host = $cachePlatform
        requestedSuite = $Suite
        play96Version = "0.3.2"
        results = @($results)
    }
    $jsonPath = Join-Path $resultsDirectory "real-core-results.json"
    $report | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $jsonPath -Encoding UTF8

    $markdown = New-Object System.Collections.Generic.List[string]
    $markdown.Add("# Real-core test results")
    $markdown.Add("")
    $markdown.Add("Generated at: $generatedAt")
    $markdown.Add("")
    $markdown.Add("Host: ``$cachePlatform``  ")
    $markdown.Add("Selection: ``$Suite``  ")
    $markdown.Add("Frontend: ``play96 0.3.2``")
    $markdown.Add("")
    $markdown.Add("| Suite | Test | Core | SHA-256 | Source | Status | Seconds |")
    $markdown.Add("| --- | --- | --- | --- | --- | --- | ---: |")
    foreach ($result in $results) {
        $markdown.Add("| $($result.suite) | ``$($result.test)`` | ``$($result.core)`` | ``$($result.sha256)`` | $($result.source) | $($result.status) | $($result.durationSeconds) |")
    }
    $markdownPath = Join-Path $resultsDirectory "real-core-results.md"
    $markdown | Set-Content -LiteralPath $markdownPath -Encoding UTF8
    Write-Host "Wrote real-core result reports: $markdownPath and $jsonPath"
}

function Invoke-CoreTest {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SuiteName,

        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [string]$Core,

        [Parameter(Mandatory = $true)]
        [string]$Source
    )

    Write-Host "Running $Name"
    $started = [DateTimeOffset]::UtcNow
    Push-Location $repoRoot
    try {
        & cargo test --all-features --test libretro_examples $Name -- --ignored --exact --nocapture
        $exitCode = $LASTEXITCODE
    }
    finally {
        Pop-Location
    }
    $duration = [Math]::Round(([DateTimeOffset]::UtcNow - $started).TotalSeconds, 2)
    $status = if ($exitCode -eq 0) { "passed" } else { "failed" }
    $script:results += [pscustomobject][ordered]@{
        suite = $SuiteName
        test = $Name
        core = [System.IO.Path]::GetFileName($Core)
        sha256 = Get-Sha256Hex -Path $Core
        source = $Source
        status = $status
        durationSeconds = $duration
    }
    if ($exitCode -ne 0) {
        Write-TestResults
        throw "Real-core integration test failed: $Name"
    }
}

if ($Suite -eq "All" -or $Suite -eq "Arduboy") {
    $env:PLAY96_ARDUBOY_CORE = Get-LibretroCore -Name "arduous_libretro"
    Invoke-CoreTest -SuiteName "Arduboy" -Name "arduboy_snake_runs_on_real_core" -Core $env:PLAY96_ARDUBOY_CORE -Source "RetroArch buildbot latest/Arduous"
}

if ($Suite -eq "All" -or $Suite -eq "GameBoy") {
    $env:PLAY96_GAMEBOY_CORE = Get-LibretroCore -Name "mgba_libretro"
    Invoke-CoreTest -SuiteName "Game Boy" -Name "gameboy_examples_run_on_real_core" -Core $env:PLAY96_GAMEBOY_CORE -Source "RetroArch buildbot latest/mGBA"
}

if ($Suite -eq "All" -or $Suite -eq "ZxSpectrum") {
    $env:PLAY96_ZX_SPECTRUM_CORE = Get-LibretroCore -Name "fuse_libretro"
    Invoke-CoreTest -SuiteName "ZX Spectrum" -Name "zx_spectrum_examples_run_on_real_core" -Core $env:PLAY96_ZX_SPECTRUM_CORE -Source "RetroArch buildbot latest/Fuse"
}

if ($Suite -eq "All" -or $Suite -eq "Cpm") {
    $env:PLAY96_CPM_CORE = Get-LibretroCore -Name "ep128emu_core_libretro"
    Invoke-CoreTest -SuiteName "CP/M" -Name "cpm_examples_run_on_real_core" -Core $env:PLAY96_CPM_CORE -Source "RetroArch buildbot latest/ep128emu"
}

if ($Suite -eq "All" -or $Suite -eq "Ez180N") {
    $env:PLAY96_EZ180N_CORE = Get-Ez180NCore
    Invoke-CoreTest -SuiteName "ez180N" -Name "ez180n_examples_run_on_real_core" -Core $env:PLAY96_EZ180N_CORE -Source "Codeberg nightly release"
}


Write-TestResults
