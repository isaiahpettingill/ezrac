# Agon Light MOS Examples

These examples target `agonlight-mos-ez80` and use built-in EZRA SDK modules from `toolchains/agonlight-mos-ez80/sdk`.

The target builds normal Agon MOS executables. The generated binary starts with a jump to the compiled entry point, stores the MOS executable marker `"MOS", 0, 1` at byte `64`, places program code at byte `69`, and returns to MOS after `main` finishes.

## Building

Build the hello example:

```sh
cargo run -- build examples/agon-mos/hello/src/main.ezra
```

This writes `main.asm`, `main.map`, and `main.bin` under the example project's `target/<target>/src` directory.

Build the interactive coffee order example:

```sh
cargo run -- build examples/agon-mos/coffee-order/src/main.ezra
```

It sets `[build].executable = "coffee-order"`, so the build writes `coffee-order.asm`, `coffee-order.map`, and `coffee-order.bin` under `examples/agon-mos/coffee-order/target/agonlight-mos-ez80/src`.

## Coffee Order Demo

The coffee example clears the screen, prints a small menu, clears MOS keyboard state, then calls `agon.mos.getkey()` for a single blocking key read.

Controls:

- `1`: order `Coffee`
- `2`: order `Latte`
- `3`: order `Monster`
- any other key: print the retry message

Expected output starts with:

```text
EZRA CAFE

1) Coffee
2) Latte
3) Monster

Pick 1-3:
```

The example is a normal MOS executable: after `main` returns, control returns to MOS. The SDK exposes `vdp.emulator_exit` for CLI-emulator automation, but normal programs should not call it.

## Ezra.toml

The coffee example project file shows the current Agon build settings:

```toml
[build]
target = "agonlight-mos-ez80"
output = "bin"
executable = "coffee-order"
```

- `target` selects the Agon MOS eZ80 target profile.
- `output` keeps the executable format as a raw `.bin` file.
- `executable` controls the artifact basename, so the output is `coffee-order.bin` instead of `main.bin`.

## Fab Agon Emulator

Fab Agon Emulator is GPL-3.0. It is not vendored or submoduled here. To test with it, install or clone it separately and point the runner at your local checkout.

On Windows PowerShell:

```powershell
$env:FAB_AGON_EMULATOR_DIR = "K:\source\fab-agon-emulator"
pwsh tools/run-fab-agon.ps1 examples/agon-mos/hello/target/agonlight-mos-ez80/src/main.bin
```

The runner copies the binary into the emulator SD card directory and starts the locally configured emulator. It does not download or redistribute emulator binaries.

To run the coffee demo instead:

```powershell
cargo run -- build examples/agon-mos/coffee-order/src/main.ezra
pwsh tools/run-fab-agon.ps1 examples/agon-mos/coffee-order/target/agonlight-mos-ez80/src/coffee-order.bin
```
