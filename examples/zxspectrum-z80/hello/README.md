# ZX Spectrum Hello

Build from the repository root:

```sh
cargo run -- build examples/zxspectrum-z80/hello/src/main.ezra
```

The resulting `zx-hello.tap` is written under the project `target/zxspectrum-z80` directory. Load it in a Spectrum emulator with `LOAD "" CODE`; if needed, start it with `RANDOMIZE USR 32768`.
