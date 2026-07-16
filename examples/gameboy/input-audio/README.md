# Game Boy Input and Audio

A DMG Game Boy example that initializes a blank background, loads a 32-sample waveform, and plays a beep plus the waveform when **A** is pressed.

- **Target:** `gameboy-dmg-lr35902`
- **Output:** 32 KiB DMG ROM (`.gb`)

## Build and run

From this directory:

```sh
ezrac build
mgba target/gameboy-dmg-lr35902/main.gb
```

The second command requires [mGBA](https://mgba.io/) on `PATH`; ensure the emulator's audio is enabled, then press **A**.
