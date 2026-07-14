# Commodore 64 Hello

A C64 screen hello-world program. It sets the border and background, displays a message, and waits for `Q`.

Build from the repository root:

```sh
cargo run --features mos6502 -- build examples/commodore64/hello/src/main.ezra
```

Load the generated `c64-hello.prg` from `target/commodore64-6502` in a C64 emulator or on hardware.
