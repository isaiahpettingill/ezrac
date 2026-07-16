# ZX Spectrum Sound

Configures AY channel A on 128K machines and pulses the ULA beeper every ten video frames while preserving a blue border, so it also demonstrates sound on 48K machines.

```sh
cargo run -- build examples/zxspectrum-z80/sound/src/main.ezra
```

The resulting `zx-sound.tap` is written under `examples/zxspectrum-z80/sound/target/zxspectrum-z80`.
