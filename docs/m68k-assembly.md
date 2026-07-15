# MC68000 assembler mode

Enable the feature-gated Motorola 68000 assembler and assemble a standalone source file:

```sh
cargo run --features m68k -- assemble --cpu m68k --target generic-m68k-bare -o program.bin program.asm
```

Output is big-endian 68000 machine code. Labels, equates, `org`, sections, and data directives are handled by EZRAC's common assembler layer. The instruction encoder is MC68000-only: it deliberately does not accept 68010+ instructions or 68020 full-format indexed extensions.

## Expressions and registers

Registers are case-insensitive: `d0`–`d7`, `a0`–`a7`, and `sp` (`a7`). `usp`, `sr`, and `ccr` are accepted only in the instructions that define them. Numbers may be decimal, `$` hexadecimal, `0x` hexadecimal, or trailing-`h` hexadecimal. Immediate operands use `#`.

All normal MC68000 size suffixes are accepted: `.b`, `.w`, and `.l`. Instructions without a defined byte/word/long encoding reject invalid suffixes. The normal default is the architecture's word form where an unsuffixed form has one.

## Effective addresses

The following forms are accepted wherever the MC68000 instruction permits them:

| Form | Meaning |
|---|---|
| `d0`, `a0`, `sp` | data/address register direct |
| `(a0)`, `(a0)+`, `-(a0)` | address indirect, postincrement, predecrement |
| `4(a0)` or `(4,a0)` | 16-bit address displacement |
| `4(a0,d1.w)` or `(4,a0,d1.l)` | brief indexed address (signed 8-bit displacement) |
| `(4,pc)` | PC-relative 16-bit displacement |
| `(4,pc,d1.w)` / `(4,pc,a1.l)` | PC-relative brief indexed address |
| `$1234.w`, `$12345678.l` | explicit absolute short or long |
| `symbol` | absolute short when its value fits 16 bits, otherwise absolute long |
| `#value` | immediate data |

For a resolved PC-relative label, the assembler emits the displacement from the extension-word PC (`instruction address + 2`). Indexed displacements are range checked to `-128..127`. Use explicit `.w`/`.l` on absolute values when the required encoding must not depend on the value during a sizing pass.

## Instruction coverage

The feature supports every official base MC68000 family and enforces the legal operand classes through the instruction encoder:

- data transfer/control: `move`, `movea`, `moveq`, `movem`, `movep`, `moveusp`, `lea`, `pea`, `link`, `unlk`, `jmp`, `jsr`, `rts`, `rtr`, `rte`, `nop`, `reset`, `stop`, `trap`, `trapv`, and `illegal`;
- integer arithmetic/compare: `add`, `adda`, `addi`, `addq`, `addx`, `sub`, `suba`, `subi`, `subq`, `subx`, `cmp`, `cmpa`, `cmpi`, `cmpm`, `mulu`, `muls`, `divu`, `divs`, and `chk`;
- logical/bit/BCD: `and`, `andi`, `or`, `ori`, `eor`, `eori`, `not`, `neg`, `negx`, `clr`, `tst`, `tas`, `nbcd`, `abcd`, `sbcd`, `btst`, `bchg`, `bclr`, and `bset`;
- branches and conditions: `bra`, `bsr`, all `b<condition>`, `db<condition>` (including `dbra`), and `s<condition>`; condition aliases `hs`/`cc`, `lo`/`cs`, and `ra`/`f` are accepted;
- shifts and rotates: register-count/immediate-count `.b/.w/.l` forms and memory forms of `asl/asr`, `lsl/lsr`, `rol/ror`, and `roxl/roxr`;
- register/system forms: `exg`, `ext`, `swap`, `move` to/from `sr`/`ccr`, and immediate `andi`/`ori`/`eori` to `sr` or `ccr`.

`MOVEM` takes a slash-separated register list, with optional ascending ranges: `d0-d2/a1/a4-a6`. For `-(An)` register-to-memory transfers the hardware's predecrement mask convention is encoded by the underlying MC68000 encoder. `MOVEP` uses `d16(An)`, for example `movep.l d0,4(a1)`.

## Validation

The assembler rejects unsupported processor extensions, illegal effective-address positions, invalid size suffixes, malformed register lists, out-of-range quick/shift/trap fields, and brief-index displacements outside the signed 8-bit range. Branch displacement selection follows the 68000 rule: a nonzero signed byte displacement uses the short encoding; zero or values outside that range use the word extension.

The feature-gated test corpus in `src/asm/m68k.rs` contains byte-level golden cases for every effective-address category and unusual encoding, a table-driven official-family smoke corpus, and invalid/boundary validation cases.
