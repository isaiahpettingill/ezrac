# Coffee Order

An interactive `agon.mos` console demo. It displays an EZRA Cafe menu, accepts one key, and reports the selected Coffee, Latte, or Monster order. Any other key asks the user to try again.

## Build

From the repository root:

```sh
cargo run -- build examples/agon-mos/coffee-order/src/main.ezra
```

This produces `examples/agon-mos/coffee-order/target/agonlight-mos-ez80/coffee-order.bin`.

## Run

With `FAB_AGON_EMULATOR_DIR` set to a local Fab Agon Emulator checkout:

```powershell
pwsh tools/run-fab-agon.ps1 examples/agon-mos/coffee-order/target/agonlight-mos-ez80/coffee-order.bin
```
