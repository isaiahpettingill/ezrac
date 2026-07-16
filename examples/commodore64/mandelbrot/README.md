# Commodore 64 Mandelbrot

A Mandelbrot escape display rendered through the C64 VIC-II high-resolution bitmap API. It draws a precomputed 40×25 escape map as native 8×8 bitmap blocks, filling the full 320×200 display without using text characters. The source also retains the Q4 fixed-point escape calculation as a reference; it is not used at runtime because the current generic C64 signed 16-bit inner-loop codegen is not reliable.

Build from the repository root:

```sh
cargo run --features mos6502 -- build examples/commodore64/mandelbrot/src/main.ezra
```

Load the generated `c64-mandelbrot.prg` from `target/commodore64-6502` in a C64 emulator or on hardware.
