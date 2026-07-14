# Console SDK Demo

A minimal `agon.console` example that clears the display, prints `EZRA console SDK`, and writes a `> ` prompt.

## Build

From the repository root:

```sh
cargo run -- build examples/agon-mos/console/src/main.ezra
```

This produces `examples/agon-mos/console/target/agonlight-mos-ez80/src/console.bin`.

## Run

With `FAB_AGON_EMULATOR_DIR` set to a local Fab Agon Emulator checkout:

```powershell
pwsh tools/run-fab-agon.ps1 examples/agon-mos/console/target/agonlight-mos-ez80/src/console.bin
```
