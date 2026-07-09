# ez180N Meteor Runner

Small EZRA game demo for the `ez180n-ez80` libretro fantasy console target.

It demonstrates:

- writing directly to the 160x112 character framebuffer through `ez180n.console`
- CP437 block and box glyphs
- SNES-style joypad input
- sound-port effects
- repeated framebuffer presents for `play96` screenshots

Build from the `ezrac` repository:

```sh
cargo run -- build examples/ez180n/meteor-runner/src/main.ezra
```

Run with `play96` after building the `ez180N` core:

```sh
../play96/build-mingw/play96 --core ../ez180N/target/release/ez180n.dll --cart examples/ez180n/meteor-runner/target/ez180n-ez80/src/meteor-runner.gaem --frames 180 --shot-every 45
```
