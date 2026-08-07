# TI-99/4A Indexed PNG Tiles

Converts the indexed `assets/tiles.png` into two 8×8 TMS9918A pattern tiles, uploads them with `ti99.sprites`, and displays both hardware sprites. The local layout keeps the cartridge header, code, and asset ROM ranges separate.

Build from the repository root:

```sh
cargo run --features tms9900 -- build examples/ti99-4a/png-assets/src/main.ezra
```
