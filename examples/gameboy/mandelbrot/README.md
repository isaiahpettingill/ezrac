# Game Boy Mandelbrot

A DMG Game Boy background-tile example. It uploads four shade tiles and fills the visible tile map with a four-shade repeating pattern using inline LR35902 assembly, then enables the background.

- **Target:** `gameboy-dmg-lr35902`
- **Output:** 32 KiB DMG ROM (`.gb`), named `gameboy-mandelbrot.gb`

## Build and run

From this directory:

```sh
ezrac build
mgba target/gameboy-dmg-lr35902/src/gameboy-mandelbrot.gb
```

The second command requires [mGBA](https://mgba.io/) on `PATH`; any Game Boy emulator can open the generated ROM.
