# Commodore 64 Indexed PNG Sprite

Converts the indexed `assets/sprite.png` into the C64 hires sprite layout, copies the 64 converted bytes from the default `$8000` asset region to sprite RAM, and displays sprite 0 with the VIC-II SDK.

Build from the repository root:

```sh
cargo run --features mos6502 -- build examples/commodore64/png-assets/src/main.ezra
```
