# DCPU-16 1.7 Assembly

Enable the optional DCPU assembler and assemble for the bare target:

```sh
cargo run --features dcpu -- assemble --target generic-dcpu-bare program.asm
```

`generic-dcpu-bare` produces a raw little-endian `.bin`. DCPU words are emitted
least-significant byte first, which is directly loadable by the
[`dcpu-16-libretro`](https://github.com/isaiahpettingill/dcpu-16-libretro) core.
Labels are case-insensitive and resolve to DCPU word addresses in instructions
and data expressions. The symbol map continues to report label locations as byte
offsets, matching the rest of the EZRA build API.

The optional `dcpu` feature uses [`dcpu16-core`](https://crates.io/crates/dcpu16-core)
for DCPU emulator-backed compiler tests. The standalone assembler stays in EZRAC.

## Examples

Build the handwritten LEM1802 example or the limited scalar EZRA source example:

```sh
cargo run --features dcpu -- build examples/dcpu-16/lem-hello/main.asm
cargo run --features dcpu -- build examples/dcpu-16/arithmetic/src/main.ezra
```

The libretro core's Standard Compatibility profile maps LEM1802 screen words at
`0x8000`. The `lem-hello` example writes directly to that screen memory.

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
next_word`. Register offsets may be expressions, such as `[table + 2 + I]`.
Integer literals may use the shared assembler's decimal, hexadecimal, binary,
and octal forms. The assembler emits next words after the instruction word in
DCPU operand order: `b`'s next word first, followed by `a`'s.

## Labels, symbols, and expressions

Both traditional and Notch-style labels are accepted, including a statement on
the same line:

```text
start:  SET A, message
:loop   SUB I, 1
```

Use `.equ NAME, expression` or `.set NAME, expression` for constants. Forward
references are supported. Constant expressions support parentheses, unary `+`,
`-`, and `~`, and these binary operators with normal precedence:

```text
* / + - << >> & ^ |
```

Symbols and `$`, the current address, have DCPU word-address values. Symbolic
literal operands always use the next-word encoding even when their final value
would fit the short-literal range. This keeps instruction sizes and label values
stable across both assembly passes. Constant-only expressions still use the
shortest literal form.

## Data

`DAT`, `DW`, `DEFW`, `WORD`, and `.short` emit 16-bit little-endian DCPU words.
Expressions and quoted strings can be mixed:

```text
message: DAT "Hello\n\0", message + 2, (8 << 2) / 4
```

Each decoded string byte occupies one DCPU word. Strings support `\\`, `\'`,
`\"`, `\0`, `\a`, `\b`, `\f`, `\n`, `\r`, `\t`, `\v`, and two-digit `\xNN`
escapes. `DB`, `DEFB`, and `BYTE` remain available when raw byte data is
required.
