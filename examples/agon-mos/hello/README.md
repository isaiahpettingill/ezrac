# Hello

A minimal Agon MOS graphics/VDU example. It clears the screen and writes `HELLO` using `agon.vdp` byte output.

## Build

From the repository root:

```sh
cargo run -- build examples/agon-mos/hello/src/main.ezra
```

This produces `examples/agon-mos/hello/target/agonlight-mos-ez80/src/main.bin`.

## Run

With `FAB_AGON_EMULATOR_DIR` set to a local Fab Agon Emulator checkout:

```powershell
pwsh tools/run-fab-agon.ps1 examples/agon-mos/hello/target/agonlight-mos-ez80/src/main.bin
```
