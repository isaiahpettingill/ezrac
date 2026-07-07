# Agon Light MOS Examples

These examples target `agonlight-mos-ez80` and use built-in EZRA SDK modules from `toolchains/agonlight-mos-ez80/sdk`.

Build the hello example:

```sh
cargo run -- build examples/agon-mos/hello/src/main.ezra
```

This writes `main.asm`, `main.map`, and `main.bin` next to the source file.

## Fab Agon Emulator

Fab Agon Emulator is GPL-3.0. It is not vendored or submoduled here. To test with it, install or clone it separately and point the runner at your local checkout.

On Windows PowerShell:

```powershell
$env:FAB_AGON_EMULATOR_DIR = "K:\source\fab-agon-emulator"
pwsh tools/run-fab-agon.ps1 examples/agon-mos/hello/src/main.bin
```

The runner copies the binary into the emulator SD card directory and starts the locally configured emulator. It does not download or redistribute emulator binaries.
