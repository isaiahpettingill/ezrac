# ZX Spectrum Hello

Build from the repository root:

```sh
cargo run -- build examples/zxspectrum-z80/hello/src/main.ezra
```

The resulting `zx-hello.tap` is written under the project `target/zxspectrum-z80` directory. It contains an auto-start BASIC loader followed by the compiled CODE block. Emulators that fast-load all tape blocks without honoring the BASIC auto-start line can start the loaded program with `RANDOMIZE USR 32768`.
