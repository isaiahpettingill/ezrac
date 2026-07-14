# Game Boy Serial Hello

A DMG Game Boy serial example that sends `Hello from EZRA on Game Boy!` followed by a newline through the Game Boy serial interface.

- **Target:** `gameboy-dmg-lr35902`
- **Output:** 32 KiB DMG ROM (`.gb`)

## Build and run

From this directory:

```sh
ezrac build
mgba target/gameboy-dmg-lr35902/src/main.gb
```

The second command requires [mGBA](https://mgba.io/) on `PATH`. Configure the emulator's serial output or link endpoint to observe the message.
