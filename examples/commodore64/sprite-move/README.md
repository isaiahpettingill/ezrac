# Commodore 64 Sprite Move

A simple VIC-II sprite example. It copies a 64-byte sprite image into free RAM at `$0340`, points sprite 0 at that data, and moves it with `W`, `A`, `S`, and `D`. Press `Q` to exit.

It demonstrates the default screen-relative sprite pointer table, sprite color, multicolor, X expansion, coordinate handling above X=255, and CIA keyboard scanning.

Build from the repository root:

```sh
cargo run --features mos6502 -- build examples/commodore64/sprite-move/src/main.ezra
```
