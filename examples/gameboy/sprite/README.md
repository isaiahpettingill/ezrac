# Game Boy Sprite

A DMG Game Boy example that uploads an 8×8 two-bit smiley tile to sprite VRAM, places it at the screen center, and enables the background and sprite layers.

- **Target:** `gameboy-dmg-lr35902`
- **Output:** 32 KiB DMG ROM (`.gb`)

## Build and run

From this directory:

```sh
ezrac build
mgba target/gameboy-dmg-lr35902/main.gb
```

The second command requires [mGBA](https://mgba.io/) on `PATH`; any Game Boy emulator can open the generated ROM.
