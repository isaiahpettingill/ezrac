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

M6809 EZRA source uses the same safe TBIR transformations as the other source backends. Power-of-two multiplication is strength-reduced before emission; other 8-bit multiplication uses the M6809 `MUL` instruction. The standalone assembler exposes the full register and addressing model.

## EZRA source-function ABI

Non-naked EZRA functions use a normal MC6809 stack frame:

- S is the descending hardware stack pointer and holds return addresses and outgoing arguments. U is the frame pointer.
- The prologue is `pshs u`, an optional `leas -frame_size,s`, then `tfr s,u`. The epilogue is an optional `leas frame_size,s`, then `puls u` followed by `rts`.
- Stack arguments sit above the saved U and return address. The first parameter occupies the highest bytes below the caller's frame; later parameters follow toward the frame base. Each parameter occupies its natural scalar size; u8 and bool values use one byte and pointers use two big-endian bytes.
- Locals that need memory, allocator spills, aggregate storage, and compiler scratch live inside the active invocation's frame at fixed offsets from U. Address-taken locals stay at those stable offsets for the whole invocation.
- A caller reserves and fills the argument area with `pshs a`/`pshs x`, calls the callee, and releases the area with `leas` after the return. No parameter, local, spill, result, or call snapshot uses fixed RAM, so direct and function-pointer recursion work without static snapshots.
- Scalar results return in A. Two-result scalar calls return the first value in A and the second in B.
- Globals, MMIO registers, strings, and embed data retain their fixed model addresses.
- Naked and interrupt functions are not supported by the source emitter.
- Inline-assembly memory operands for automatic locals expand to frame-relative operands.
