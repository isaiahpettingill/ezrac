# Commodore 64 Raster Bands

A minimal raster-synchronized VIC-II effect. It waits for several raster lines and changes the border color at each one, producing horizontal color bands. Press `Q` to exit.

Build from the repository root:

```sh
cargo run --features mos6502 -- build examples/commodore64/raster-bands/src/main.ezra
```
