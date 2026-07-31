# NES Hello World

This is a raw assembly example for the `nes-2a03` target. It is based on Thomas Wesley Scott's [nes-hello-world](https://github.com/thomaslantern/nes-hello-world) example.

Upstream license: “Feel free to copy this code, modify it, and use it for either personal or commercial purposes.” The source is kept as close to the upstream example as possible; EZRA's assembler accepts its `org`, `db`, `dw`, and `ds` directives directly.

Build it from the repository root:

```sh
cargo run -- build examples/nes-2a03/hello-world/helloWorld.asm
```

The resulting `target/nes-2a03/helloWorld.nes` is an iNES NROM-128 image. Run it in an NES emulator such as Mesen, FCEUX, or Nestopia.
