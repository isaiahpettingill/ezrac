# MOS 6502 assembler mode

EZRAC can assemble standalone source for the official NMOS MOS 6502 instruction set:

```sh
ezrac assemble --cpu 6502 --target generic-6502-bare --base 8000h -o program.bin program.asm
```

The assembler is deliberately limited to the documented NMOS 6502 ISA. It is suitable for the MOS 6502 and compatible 6510 CPU used by the Commodore 64. It does **not** accept 65C02 extensions or undocumented NMOS opcodes.

## Syntax and addressing

* Labels use `name:` and label lookup is case-insensitive.
* The normal assembler provides equates, data directives, sections, `org`, and expressions. `$` in an expression denotes the current program counter.
* Numbers may use decimal, `0x` hexadecimal, or trailing-`h` hexadecimal syntax. Within 6502 operands, `$` prefixes hexadecimal values: `lda $d020`.
* Official forms are implied (`clc`), accumulator (`asl a`), immediate (`lda #$12`), zero page (`lda $12`), zero-page indexed (`lda $12,x`, `ldx $12,y`), absolute (`lda $1234`), absolute indexed (`lda $1234,x` or `lda $1234,y`), indirect jump (`jmp ($1234)`), indexed-indirect (`lda ($12,x)`), indirect-indexed (`lda ($12),y`), and relative branches (`bne label`). Whitespace around addressing commas is accepted.
* Literal addresses in `$00` through `$ff` select zero-page encoding when that opcode has one; larger literals select absolute encoding. Symbol references always select absolute encoding, even if their resolved address is in zero page. This stable policy keeps the sizing and final passes identical. Use a literal zero-page address when a zero-page encoding is required.
* Immediate, zero-page, and indirect pointer operands are unsigned 8-bit values. Absolute addresses and indirect jump targets are unsigned 16-bit values and are emitted little-endian.
* Branch targets are 16-bit addresses. Branch displacements are signed 8-bit offsets from the following instruction, including normal 16-bit program-counter wraparound at `$ffff`.

## Official instruction coverage

All 151 documented NMOS 6502 opcodes are accepted, including every official addressing form for `adc`, `and`, `asl`, `bit`, branches, `cmp`, `cpx`, `cpy`, `dec`, `eor`, `inc`, `lda`, `ldx`, `ldy`, `lsr`, `ora`, `rol`, `ror`, `sbc`, `sta`, `stx`, and `sty`; the documented control, flag, stack, transfer, and interrupt instructions are included as well.

The assembler rejects 65C02-only instructions and forms such as `bra`, `stz`, `phx`, `plx`, `phy`, `ply`, `trb`, `tsb`, `wai`, `stp`, `bit #imm`, `inc a`, and `jmp (abs,x)`. It also rejects undocumented NMOS opcodes and mnemonics such as `lax`, `sax`, `dcp`, `isc`, `slo`, `rla`, `sre`, `rra`, and unofficial multi-byte `nop` forms.

The implementation has table-driven golden tests for every official opcode/addressing-form encoding, label policy, operand ranges, branch limits and wraparound, and excluded 65C02/undocumented forms.
