# Mandelbrot

A compact `agon.vdp` graphics demo. It enters mode 8, clears the graphics area, and plots a 160×120 color pattern calculated from each pixel coordinate.

## Build

From the repository root:

```sh
cargo run -- build examples/agon-mos/mandelbrot/src/main.ezra
```

This produces `examples/agon-mos/mandelbrot/target/agonlight-mos-ez80/agon-mandelbrot.bin`.

## Run

With `FAB_AGON_EMULATOR_DIR` set to a local Fab Agon Emulator checkout:

```powershell
pwsh tools/run-fab-agon.ps1 examples/agon-mos/mandelbrot/target/agonlight-mos-ez80/agon-mandelbrot.bin
```
