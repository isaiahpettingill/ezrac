# SNES raw assembly hello

This is a handwritten LoROM startup example for the SNES Ricoh 5A22. It enters native 65C816 mode, forces a blank screen during startup, sets a blue backdrop through CGRAM, and waits in a frame loop.

Build it from the repository root after the `snes-5a22` target profile and ROM packager are registered:

```sh
cargo run -- build examples/snes-5a22/hello-world/hello.asm
```

The source includes the LoROM internal header and native/emulation vectors. It uses only text source; there are no external or binary assets.

The manifest selects `65c816` explicitly because the 5A22 shares the 65C816 instruction encoding. The current checkout does not register `snes-5a22` in the existing compiler sources, and this example does not change those files.
