# Tutorial: Agon MOS coffee order

This walkthrough builds and runs the checked-in interactive Agon MOS example. It demonstrates imports, constants, functions, keyboard input, loops, and target SDK calls without requiring inline assembly.

## Build

From the repository root:

```sh
cargo run -- build examples/agon-mos/coffee-order/src/main.ezra
```

The project configuration in `examples/agon-mos/coffee-order/Ezra.toml` selects `agonlight-mos-ez80` and names the output `coffee-order`. The binary is:

```text
examples/agon-mos/coffee-order/target/agonlight-mos-ez80/coffee-order.bin
```

## Run in Fab Agon Emulator

Fab Agon Emulator is not vendored. Set `FAB_AGON_EMULATOR_DIR` to a local checkout or release, then run:

```powershell
pwsh tools/run-fab-agon.ps1 examples/agon-mos/coffee-order/target/agonlight-mos-ez80/coffee-order.bin
```

The program prints a menu, waits for a key, and accepts `1`, `2`, or `3`. Any other key prints an error and shows the menu again.

## What to read

The source imports `agon.mos` and uses `mos.puts`, `mos.putc`, `mos.getkey`, and `mos.clear_key_state`. The loop exits by returning from `main`; normal MOS programs should return to MOS rather than write emulator-only exit ports. See [built-in SDKs](../sdk.md) and [Agon target details](../targets-and-layouts.md#default-layouts).
