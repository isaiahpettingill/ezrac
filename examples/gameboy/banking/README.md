# Game Boy Explicit Banking

This example builds an MBC5 Game Boy ROM with explicit source banking enabled.
`bank_marker`, `checker_tile`, and `draw_banked_tile` live in ROM bank 2.
`main` is resident in bank 0 and calls the banked function through EZRA's
resident far-call trampoline. The banked routine copies its tile to VRAM and
shows a deterministic checkerboard background.

```sh
ezrac build src/main.ezra
```

The generated ROM is `target/gameboy-dmg-lr35902/main.gb`.
