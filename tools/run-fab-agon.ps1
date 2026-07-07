param(
    [Parameter(Mandatory = $true)]
    [string] $Program,

    [string] $EmulatorDir = $env:FAB_AGON_EMULATOR_DIR,
    [string] $Firmware = "quark"
)

if (-not $EmulatorDir) {
    Write-Error "Set FAB_AGON_EMULATOR_DIR to a local Fab Agon Emulator checkout or release directory."
    exit 2
}

$emulator = Join-Path $EmulatorDir "fab-agon-emulator.exe"
$sdcard = Join-Path $EmulatorDir "sdcard"

if (-not (Test-Path $emulator)) {
    Write-Error "Fab Agon Emulator executable not found: $emulator"
    exit 2
}

if (-not (Test-Path $Program)) {
    Write-Error "Program binary not found: $Program"
    exit 2
}

if (-not (Test-Path $sdcard)) {
    New-Item -ItemType Directory -Path $sdcard | Out-Null
}

$dest = Join-Path $sdcard (Split-Path $Program -Leaf)
Copy-Item $Program $dest -Force

& $emulator --sdcard $sdcard --firmware $Firmware
exit $LASTEXITCODE
