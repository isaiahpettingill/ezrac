# ez180N Font Showcase

Displays all 256 byte slots as a 16x16 grid of glyphs. There is no control
header or byte label: each font has its own character mapping, so the same
byte can represent a different glyph after changing fonts.

Controls:

- Left/right: cycle the text color through the ANSI 256-color palette.
- Up/down: cycle the background color through the ANSI 256-color palette.
- A/B: cycle forward/backward through the 24 bundled 8x8 fonts.

Build from the ezrac repository root:

```sh
cargo run -- build examples/ez180n/font-showcase/src/main.ezra
```

Run it with the installed ez180N core and play96:

```sh
../play96/target/release/play96-cli.exe --core ../ez180N/target/release/ez180n.dll --cart examples/ez180n/font-showcase/target/ez180n-ez80/font-showcase.gaem --frames 120
```
