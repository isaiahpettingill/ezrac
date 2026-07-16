# Game Boy Background

A DMG Game Boy example that embeds a checkerboard tile, copies it to VRAM, clears the background tile map, and enables the background layer.

- **Target:** `gameboy-dmg-lr35902`
- **Output:** 32 KiB DMG ROM (`.gb`)

## Build and run

From this directory:

```sh
ezrac build
mgba target/gameboy-dmg-lr35902/main.gb
```

The second command requires [mGBA](https://mgba.io/) on `PATH`; any Game Boy emulator can open the generated ROM.
