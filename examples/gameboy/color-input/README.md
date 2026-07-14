# Game Boy Color Input

A Game Boy Color example that displays a repeating four-tile background and changes its palette or scroll position in response to joypad input.

- **Target:** `gameboy-color-lr35902`
- **Output:** 32 KiB CGB ROM (`.gbc`)

## Build and run

From this directory:

```sh
ezrac build
mgba target/gameboy-color-lr35902/src/main.gbc
```

The second command requires [mGBA](https://mgba.io/) on `PATH`; use a Game Boy Color-capable emulator.

## Controls

1. **A** switches from the warm palette to the cool palette.
2. **Right** scrolls the background right by 8 pixels.
3. **Left** scrolls it back by 8 pixels.
4. **B** restores the warm palette.
