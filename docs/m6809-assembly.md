# Motorola 6809 assembler mode

EZRAC can assemble standalone Motorola 6809 source:

```sh
cargo run --features m6809 -- assemble --cpu m6809 --target bare-m6809 --base 8000h -o program.bin program.asm
```

Enable the optional `m6809` Cargo feature. `bare-m6809` also accepts EZRA source through the shared HIR → TBIR pipeline and can run assembled test images with the built-in 6809 emulator.

## Syntax

* Labels use `name:` and are case-insensitive when referenced.
* Equates use `name equ expression`, `.equ name, expression`, or `name = expression`.
* Data directives use `db`/`byte` for bytes and `dw`/`word` for big-endian 16-bit words.
* Placement directives use `org expression` and `section name` through the normal EZRAC assembler layout path.
* Numeric literals may be decimal, `0x` hex, trailing-`h` hex, `$` hex, or `%` binary.
* Addressing modes include inherent, immediate, direct, extended, short and long relative branches, and M6809 indexed forms using X, Y, U, S, or PC.
* Indexed forms include 5-, 8-, and 16-bit offsets, accumulator offsets (`a,x`, `b,y`, `d,u`), auto-increment/decrement (`x+`, `x++`, `-x`, `--x`), PC-relative forms (`label,pcr`), and bracketed indirection (`[$1234]`, `[8,s]`).
* `exg` and `tfr` use the M6809 register names `d`, `x`, `y`, `u`, `s`, `pc`, `a`, `b`, `cc`, and `dp`. `pshs`, `puls`, `pshu`, and `pulu` accept comma-separated register lists.
* M6800 accumulator spellings such as `ldaa`, `staa`, `ldab`, and `stab` remain accepted for source compatibility.

## Instruction coverage

The assembler covers the official MC6809 instruction set, including the A/B accumulator and memory operations, D accumulator operations, X/Y/U/S comparisons and loads/stores, `LEA`, `MUL`, `EXG`, `TFR`, stack masks, short and long branches, `SWI2`/`SWI3`, condition-code operations, indexed indirection, and the M6800-compatible aliases.

M6809 EZRA source uses the same safe TBIR transformations as the other source backends. Power-of-two multiplication is strength-reduced before emission; other 8-bit multiplication uses the M6809 `MUL` instruction. The source backend keeps the existing scalar RAM ABI, while the standalone assembler exposes the full register and addressing model.
