# ZX Spectrum Indexed PNG Tiles

Converts the indexed `assets/tiles.png` into four 8×8 Spectrum bitmap tiles, writes them through `zx.screen`, and sets their ULA attributes.

Build from the repository root:

```sh
cargo run -- build examples/zxspectrum-z80/png-assets/src/main.ezra
```
