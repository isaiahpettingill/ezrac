# ZX Spectrum Mandelbrot

This example renders a compact Mandelbrot-inspired pattern into the ZX Spectrum bitmap and sets a bright cyan-on-blue attribute layer.

Build from the repository root:

```sh
cargo run -- build examples/zxspectrum-z80/mandelbrot/src/main.ezra
```

The build writes `zx-mandelbrot.tap` under the example's `target/zxspectrum-z80` directory. The tape contains an auto-start BASIC loader followed by the compiled CODE block. If an emulator fast-loads the tape without running the loader, start the loaded program with `RANDOMIZE USR 32768`.
