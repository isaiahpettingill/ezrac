# DCPU-16 examples

These examples target the raw, little-endian `generic-dcpu-bare` image format. They run directly in the [DCPU-16 libretro core](https://github.com/isaiahpettingill/dcpu-16-libretro), which maps the LEM1802 display to word address `0x8000` in Standard Compatibility mode.

Build from the repository root:

```sh
cargo run --features dcpu -- build examples/dcpu-16/lem-hello/main.asm
cargo run --features dcpu -- build examples/dcpu-16/arithmetic/src/main.ezra
cargo run --features dcpu -- build examples/dcpu-16/sdk-hello/main.asm
```

The `lem-hello` assembly example uses `toolchains/generic-dcpu-bare/sdk/asm/dcpu.inc` to write a title and counter to LEM1802 screen memory. `sdk-hello` configures the display, keyboard, clock, and speaker through SDK macros, then writes a short message and waits for keys. The source example imports the built-in `dcpu.lem1802` module and stays within EZRAC's current DCPU source-backend limits: a parameterless `main`, scalar locals, and straight-line arithmetic.

Vendor `toolchains/generic-dcpu-bare/sdk/asm/dcpu.inc` into handwritten assembly projects. It provides device-slot constants plus macros for generic `HWI`, LEM1802 text output, keyboard reads, clock setup, and two-channel speaker frequencies.
