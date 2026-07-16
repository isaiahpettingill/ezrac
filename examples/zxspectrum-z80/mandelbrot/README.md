# ZX Spectrum Mandelbrot

This example renders a 32×24 Mandelbrot escape map into the ZX Spectrum bitmap, expanding each sample to an 8×8 pixel cell and setting a bright cyan-on-blue attribute layer. The source retains the Q4 fixed-point escape calculation as a reference; the displayed map is precomputed so it renders immediately on a stock Spectrum.

Build from the repository root:

```sh
cargo run -- build examples/zxspectrum-z80/mandelbrot/src/main.ezra
```

The build writes `zx-mandelbrot.tap` under the example's `target/zxspectrum-z80` directory. The tape contains an auto-start BASIC loader followed by the compiled CODE block. If an emulator fast-loads the tape without running the loader, start the loaded program with `RANDOMIZE USR 32768`.
