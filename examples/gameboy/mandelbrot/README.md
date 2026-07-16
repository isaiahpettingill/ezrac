# Game Boy Mandelbrot

A DMG Game Boy background-tile example. It uploads four shade tiles and copies precomputed Mandelbrot escape-time samples into the visible background tile map, then enables the background.

- **Target:** `gameboy-dmg-lr35902`
- **Output:** 32 KiB DMG ROM (`.gb`), named `gameboy-mandelbrot.gb`

## Build and run

From this directory:

```sh
ezrac build
mgba target/gameboy-dmg-lr35902/gameboy-mandelbrot.gb
```

The second command requires [mGBA](https://mgba.io/) on `PATH`; any Game Boy emulator can open the generated ROM.
