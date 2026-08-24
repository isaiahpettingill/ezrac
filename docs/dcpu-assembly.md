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

## EZRA source ABI

DCPU-16 source code uses word-addressed memory. `AssemblyOptions.stack_top` is a
byte address; it must be even and no greater than `0x1FFFE`, and the emitter
converts it to the DCPU word value used to initialize `SP`. The stack grows
downward one word at a time. Compiler-managed automatic storage is never a
fixed global slot.

A compiled function saves `J` at `[J+0]` and the return PC at `[J+1]`. Scalar
parameters are at `[J+2]`, `[J+3]`, and so on in source order. Register locals
use `C`, `X`, `Y`, and `Z` when safe; values live across a call, inline assembly,
or re-entry spill to negative `J`-relative frame slots. Address-taken locals and
aggregate locals always have stable frame storage. Frame displacements are
signed 16-bit values, so oversized frames are rejected.

Callers evaluate arguments and push them right-to-left. A callee returns its
first scalar result in `A` and its second scalar result in `EX`. Aggregate
parameters and aggregate returns are not part of this source ABI; use pointers
and caller-owned storage. Explicit globals, MMIO, embeds, and static SDK state
remain at their configured fixed addresses.

The source backend does not implement compiler-managed `interrupt` functions.
Interrupt entry, register preservation, acknowledgment, and return must be
written as a `naked` assembly wrapper using the platform's vector and hardware
rules. A naked wrapper has no compiler-generated frame or operand binding and
must preserve the DCPU stack ABI before calling compiled source.

## Examples

Build the handwritten LEM1802 example or the limited scalar EZRA source example:

```sh
cargo run --features dcpu -- build examples/dcpu-16/lem-hello/main.asm
cargo run --features dcpu -- build examples/dcpu-16/arithmetic/src/main.ezra
```

The libretro core's Standard Compatibility profile maps LEM1802 screen words at
`0x8000`. The `lem-hello` example uses the vendorable SDK macros to write that
screen memory.

## Standard Machine SDK

`toolchains/generic-dcpu-bare/sdk/dcpu/` contains built-in `dcpu.*` modules with
typed constants and source-level device wrappers. Use them from ordinary EZRA
source; the compiler expands the wrappers into the required `HWI` sequence:

```ezra
import dcpu.clock
import dcpu.keyboard
import dcpu.lem1802
import dcpu.speaker

fn main() {
    lem1802.map_screen(0x8000)
    lem1802.set_border(9)
    keyboard.clear()
    clock.set_rate(60)
    speaker.set_left(440)
}
```

The vendorable assembly macro SDK remains available for handwritten assembly.
It covers generic `HWI`, LEM1802 setup and text cells, keyboard queue reads,
clock setup, and stereo speaker frequencies. Use `%dcpu_hwi(device)` for other
commands after loading `A` through `Z` as required by that device.

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
