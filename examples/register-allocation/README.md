# Register allocation test case

This source fixture keeps scalar locals live at the same time, keeps one value live across a call, and creates two nonoverlapping locals. It is used to inspect physical-register allocation and spill-slot reuse across source backends. Backend unit tests cover address-taken locals because the M6800 source subset does not support pointers.

Emit assembly for the configured targets with:

```sh
cargo run --all-features -- emit-asm --target agonlight-mos-ez80 examples/register-allocation/main.ezra
cargo run --all-features -- emit-asm --target commodore64-6502 examples/register-allocation/main.ezra
cargo run --all-features -- emit-asm --target gameboy-dmg-lr35902 examples/register-allocation/main.ezra
cargo run --all-features -- emit-asm --target bare-avr examples/register-allocation/main.ezra
cargo run --all-features -- emit-asm --target bare-i8086 examples/register-allocation/main.ezra
cargo run --all-features -- emit-asm --target bare-tms9900 examples/register-allocation/main.ezra
cargo run --all-features -- emit-asm --target generic-dcpu-bare examples/register-allocation/main.ezra
cargo run --all-features -- emit-asm --target generic-m68k-bare examples/register-allocation/main.ezra
cargo run --all-features -- emit-asm --target bare-m6800 examples/register-allocation/main.ezra
cargo run --all-features -- emit-asm --target bare-m6809 examples/register-allocation/main.ezra
```

Expected allocator behavior:

- AVR uses `r2`–`r15` for eligible scalar locals.
- 8086 uses `BP` for an eligible 16-bit local; this fixture mainly exercises byte spills.
- TMS9900 uses `R6`–`R8` for eligible scalar locals.
- DCPU-16 uses `C`, `X`, `Y`, and `Z` for eligible scalar locals.
- M68k uses `A2`–`A6` for eligible pointer locals.
- eZ80/Z80, LR35902, MOS 6502, M6800, and M6809 color memory spill slots because their current instruction selectors use all generally available scalar registers.
- Values live across calls remain in safe memory storage. Backend tests also verify that address-taken locals spill.
