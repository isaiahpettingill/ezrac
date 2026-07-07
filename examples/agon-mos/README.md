# Agon Light MOS Examples

These examples target `agonlight-mos-ez80` and use built-in EZRA SDK modules from `toolchains/agonlight-mos-ez80/sdk`.

Build the hello example:

```sh
cargo run -- build examples/agon-mos/hello/src/main.ezra
```

This writes `main.asm`, `main.map`, and `main.bin` under the example project's `target/<target>/src` directory.

Build the interactive coffee order example:

```sh
cargo run -- build examples/agon-mos/coffee-order/src/main.ezra
```

It sets `[build].executable = "coffee-order"`, so the build writes `coffee-order.asm`, `coffee-order.map`, and `coffee-order.bin` under `examples/agon-mos/coffee-order/target/agonlight-mos-ez80/src`. It clears the MOS keyboard state before calling `agon.mos.getkey()` and lets you order `Coffee`, `Latte`, or `Monster`.

The example is a normal MOS executable: after `main` returns, control returns to MOS. The SDK exposes `vdp.emulator_exit` for CLI-emulator automation, but normal programs should not call it.

## Fab Agon Emulator

Fab Agon Emulator is GPL-3.0. It is not vendored or submoduled here. To test with it, install or clone it separately and point the runner at your local checkout.

On Windows PowerShell:

```powershell
$env:FAB_AGON_EMULATOR_DIR = "K:\source\fab-agon-emulator"
pwsh tools/run-fab-agon.ps1 examples/agon-mos/hello/target/agonlight-mos-ez80/src/main.bin
```

The runner copies the binary into the emulator SD card directory and starts the locally configured emulator. It does not download or redistribute emulator binaries.
