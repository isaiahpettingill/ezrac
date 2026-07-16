# Commodore 64 KERNAL Console

A compact KERNAL jump-table example. It clears the editor screen, writes PETSCII characters with `c64.kernal`, and consumes the KERNAL keyboard buffer until `Q` is pressed.

Build from the repository root:

```sh
cargo run --features mos6502 -- build examples/commodore64/kernal-console/src/main.ezra
```

The generated `c64-kernal-console.prg` is under the example's `target/commodore64-6502` directory.
