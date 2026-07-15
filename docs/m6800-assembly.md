# Motorola 6800 assembler mode

EZRAC can assemble standalone Motorola 6800 source without running EZRA language code generation:

```sh
ezrac assemble --cpu m6800 --target bare-m6800 --base 8000h -o program.bin program.asm
```

The `m6800` assembler is intentionally separate from any future EZRA m6800 backend or emulator integration.

## Syntax

* Labels use `name:` and are case-insensitive when referenced.
* Equates use `name equ expression`, `.equ name, expression`, or `name = expression`.
* Data directives use `db`/`byte` for bytes and strings, and `dw`/`word` for big-endian 16-bit words.
* Placement directives use `org expression` and `section name` through the normal EZRAC assembler layout path.
* Numeric literals may be decimal, `0x` hex, trailing-`h` hex, `$` hex in m6800 operands, or `%` binary in m6800 operands.
* Expressions support atoms joined by whitespace-separated `+` and `-`; `$` means the current program counter.
* Addressing modes use standard M6800 forms: inherent (`inx`), immediate (`ldaa #42h`), direct (`staa <20h`), indexed (`ldaa 4,x` or `ldaa 4, x`), extended (`jmp >C000h`), and relative branches (`bne label`). Use `<` or `>` only to force direct or extended encoding when a symbol is not enough to infer width during the sizing pass.
* Direct and indexed operands are unsigned 8-bit values; extended operands and immediate operands for `cpx`, `lds`, and `ldx` are big-endian 16-bit values. Other immediate operands are 8-bit. Branches are signed 8-bit displacements from the instruction following the branch, with 16-bit M6800 program addresses.
* `bhs`/`bcc`, `blo`/`bcs`, and `lsl`/`asl` are accepted equivalent spellings.

## Instruction coverage

The assembler accepts the complete official Motorola 6800 ISA. This includes all inherent accumulator, stack, and condition-code operations; `bra`, `brn`, all conditional branches, and `bsr`; indexed and extended memory unary/control operations; and the A, B, X, and stack load/store/arithmetic families in every addressing mode defined by the manual. Store instructions do not accept immediate operands, and memory unary/control operations do not have direct forms.

The implementation has table-driven golden tests for every mnemonic/addressing-form encoding, aliases, operand limits, relative branch limits, labels, equates, and invalid forms. Non-6800 instructions and invalid addressing modes are rejected with diagnostics such as `assembler does not support M6800 instruction ...`.
