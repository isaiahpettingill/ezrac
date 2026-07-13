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
* Addressing modes use standard M6800 forms: inherent (`inx`), immediate (`ldaa #42h`), direct (`staa <20h`), indexed (`ldaa 4,x`), extended (`jmp >C000h`), and relative branches (`bne label`). Use `<` or `>` to force direct or extended encoding when a symbol is not enough to infer width during the sizing pass.

## Instruction coverage

The assembler accepts the full Motorola 6800 instruction set: inherent accumulator/stack/flag operations, all conditional and unconditional branches, memory shifts/rotates/arithmetic/control operations in indexed and extended forms, and accumulator/index/stack load-store/arithmetic instructions across immediate, direct, indexed, and extended modes where valid.

Non-6800 instructions and invalid addressing modes are rejected with diagnostics such as `assembler does not support M6800 instruction ...`.
