# ZX Spectrum Input

Scans the `Shift/Z/X/C/V` keyboard row and a Kempston joystick. Hold `Z` or joystick left for a red border, `X` or joystick right for green, and Kempston fire for yellow.

```sh
cargo run -- build examples/zxspectrum-z80/input/src/main.ezra
```

The resulting `zx-input.tap` is written under `examples/zxspectrum-z80/input/target/zxspectrum-z80`.
