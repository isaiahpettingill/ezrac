# TI-99/4A Mandelbrot Tile Study

This TI-99/4A cartridge example uses TMS9900 fixed-point multiply instructions to generate a repeating Mandelbrot-inspired escape palette in the TMS9918A name table.

Build from this directory:

```sh
cargo run --manifest-path ../../../Cargo.toml --features tms9900 -- build
```

The output is `target/ti99-4a-tms9900/ti99-mandelbrot.bin`, a raw one-bank cartridge ROM beginning at `>6000`.
