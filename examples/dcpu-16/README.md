# DCPU-16 examples

These examples target the raw, little-endian `generic-dcpu-bare` image format. They run directly in the [DCPU-16 libretro core](https://github.com/isaiahpettingill/dcpu-16-libretro), which maps the LEM1802 display to word address `0x8000` in Standard Compatibility mode.

Build from the repository root:

```sh
cargo run --features dcpu -- build examples/dcpu-16/lem-hello/main.asm
cargo run --features dcpu -- build examples/dcpu-16/arithmetic/src/main.ezra
```

The assembly example writes a title and counter to LEM1802 screen memory. The source example stays within EZRAC's current DCPU source-backend limits: a parameterless `main`, scalar locals, and straight-line arithmetic.
