# DCPU-16 1.7 Assembly

Enable the optional DCPU assembler and assemble for the bare target:

```sh
cargo run --features dcpu -- assemble --target generic-dcpu-bare program.asm
```

`generic-dcpu-bare` produces a raw little-endian `.bin`. DCPU words are emitted
least-significant byte first. Labels are case-insensitive and resolve to DCPU
word addresses (the byte address divided by two).

## Instructions

The standalone assembler supports all DCPU-16 1.7 basic opcodes:

```text
SET ADD SUB MUL MLI DIV DVI MOD MDI AND BOR XOR SHR ASR SHL
IFB IFC IFE IFN IFG IFA IFL IFU ADX SBX STI STD
```

It also supports every 1.7 special opcode:

```text
JSR INT IAG IAS RFI IAQ HWN HWQ HWI
```

Basic instructions use `opcode b, a`; special instructions use exactly one `a`
operand. As required by the DCPU encoding, literal short forms (`-1`, `0` through
`30`) are valid only in the `a` position. `PUSH` is only valid as `b`, and `POP`
is only valid as `a`.

## Operands

The following DCPU operand forms are accepted:

```text
A B C X Y Z I J
[A] [B] [C] [X] [Y] [Z] [I] [J]
[next_word + register]   [register + next_word]
PUSH POP PEEK PICK next_word SP PC EX
[next_word]
next_word
-1, 0 through 30
```

`[SP]` is accepted as `PEEK`; `[SP + next_word]` is accepted as `PICK
next_word`. Integer literals may be decimal, `0x`-prefixed hexadecimal, or
`h`-suffixed hexadecimal. The assembler emits next words after the instruction
word in DCPU operand order: `b`'s next word first, followed by `a`'s.
