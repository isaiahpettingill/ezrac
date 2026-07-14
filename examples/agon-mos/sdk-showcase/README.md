# Agon SDK Showcase

A visual tour of the Agon SDK wrappers. It uses console and VDP drawing APIs, enables the mouse, displays keyboard and mouse status, clears a VDP buffer, and pulses GPIO port B.

## Build

From the repository root:

```sh
cargo run -- build examples/agon-mos/sdk-showcase/src/main.ezra
```

This produces `examples/agon-mos/sdk-showcase/target/agonlight-mos-ez80/src/sdk-showcase.bin`.

## Run

With `FAB_AGON_EMULATOR_DIR` set to a local Fab Agon Emulator checkout:

```powershell
pwsh tools/run-fab-agon.ps1 examples/agon-mos/sdk-showcase/target/agonlight-mos-ez80/src/sdk-showcase.bin
```
