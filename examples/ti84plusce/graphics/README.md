# TI-84 Plus CE Graphics

Build a protected assembly program:

```sh
cargo run -- build examples/ti84plusce/graphics/src/main.ezra
```

The output is `examples/ti84plusce/graphics/target/ti84plusce-ez80/EZRAGFX.8xp`.
Transfer it with TI Connect CE or CEmu, then run `Asm(prgmEZRAGFX)`. The program draws red, green, blue, and white quadrants using the calculator's native 16-bit framebuffer, waits for a key through TI-OS, and returns cleanly.

CEmu requires a user-supplied TI-84 Plus CE ROM. The repository does not include or redistribute calculator ROMs.
