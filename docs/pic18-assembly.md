# PIC18 target and assembler

EZRAC supports the classic PIC18 instruction set through the `pic18` Cargo feature. The initial source and assembly target is `generic-pic18-bare` and produces Intel HEX.

Extended instruction-set mode (`XINST`) is intentionally disabled. Targets containing `extended` or `xinst` are rejected so classic and extended code cannot be mixed by accident.

## Build

```sh
cargo run -- build --target generic-pic18-bare examples/pic18/gcd/src/main.ezra
cargo run -- assemble --cpu pic18 --output program.bin program.asm
```

Project builds use `output = "hex"`. Standalone assembly may still request raw bytes.

## Architecture model

- Program memory is a separate 21-bit, byte-addressed space. Instructions are aligned to two bytes.
- Data pointers are 16-bit FSR values. The classic directly addressable data space is 4 KiB.
- Access-bank file operands below `0x60` select `0x000..0x05f`; values at or above `0x60` select `0xf60..0xfff`.
- Banked file operands use the low four bits of `BSR`.
- Calls and returns use the 31-entry hardware return stack.
- Reset, high-priority interrupt, and low-priority interrupt vectors are at byte addresses `0x0000`, `0x0008`, and `0x0018`.
- Source code starts at `0x0020` after the vectors.

The generic profile does not select a concrete device. Device SFR maps, peripherals, configuration words, flash size, and RAM holes must be supplied by a future device profile or custom layout.

## Source ABI

The source backend uses the full AVR HIR/TBIR lowering and translates its byte-register ABI into compiler-private PIC18 data bytes:

- virtual byte registers `r0..r31` map to access RAM `0x20..0x3f`
- AVR X and Z pointer operations map through FSR0 and FSR1
- FSR2 is the compiler data stack
- scalar return bytes follow the existing AVR lowering convention
- program labels are byte addresses; data pointers are FSR values

This gives PIC18 the same source-language coverage as the AVR lowering. The emitted assembly is native classic PIC18 assembly.

## Emulator-backed tests

The `pic18-emulator` crate runs PIC18 tests in process. The test-host data addresses are:

- `0xf70`: changed debug sequence
- `0xf71`: debug byte
- `0xf72`: result code
- `0xf73`: nonzero value halts the test

The emulator is maintained separately at <https://github.com/isaiahpettingill/pic18-emulator> and published on crates.io.

## Current limits

- no concrete-device SDK or peripheral model
- no configuration-word emission
- no flash table writes
- no extended instruction-set mode
- no cycle-accurate device timing
