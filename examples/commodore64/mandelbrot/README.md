# Commodore 64 Mandelbrot

A low-resolution Mandelbrot escape-palette display written directly to the C64 VIC-II screen and colour RAM, with a 6502 inline-assembly inner loop.

Build from the repository root:

```sh
cargo run --features mos6502 -- build examples/commodore64/mandelbrot/src/main.ezra
```

Load the generated `c64-mandelbrot.prg` from `target/commodore64-6502` in a C64 emulator or on hardware.
