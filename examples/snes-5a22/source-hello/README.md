# SNES source hello

This example is a small Ezra source program for the Ricoh 5A22 and the SNES PPU. It initializes the CPU-side hardware, clears the low RAM mirror, sets a blue CGRAM backdrop, enables automatic controller reads, and polls the first controller once per frame.

Build it from the repository root after the `snes-5a22` target profile is registered:

```sh
cargo run -- build examples/snes-5a22/source-hello/src/main.ezra
```

The example has no external assets. `snes.ppu` writes the 15-bit backdrop color as two CGRAM bytes, so the program can show a color screen without a tile or binary asset.

The manifest names the requested `snes-5a22` target and also lists the new SDK root explicitly. This checkout does not yet register that target in the existing compiler sources; adding that registration would require editing files outside the allowed paths.
