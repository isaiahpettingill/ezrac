# TMS9900 Assembly

The optional `tms9900` feature enables the standalone TMS9900 assembler and the generic `bare-tms9900` raw-binary target. It is intended for hand-written assembly; EZRA source compilation, a TI-99/4A cartridge format, SDK, and emulator-backed `ezrac test` runner are not included.

## Build

From the repository root:

```sh
cargo run --features tms9900 -- assemble --target bare-tms9900 --base 0xA000 --output program.bin program.asm
```

`build --input-kind assembly --target bare-tms9900` uses the same assembler and writes a raw `.bin` artifact.

## Syntax

Instructions are case-insensitive. Words are emitted in the TMS9900's big-endian byte order. Integer literals may be decimal, `0x` hexadecimal, `h`-suffixed hexadecimal, or TI-style `>FFFF` hexadecimal. Labels resolve case-insensitively.

Registers use `R0` through `R15`. The supported general-address forms are:

```asm
r1              ; register direct
*r1             ; register indirect
*r1+            ; register indirect with auto-increment
@>8300          ; symbolic/direct memory address
@buffer(r4)     ; indexed memory address
```

The assembler implements the complete original TI TMS9900 instruction set:

- two-operand word and byte operations: `SZC`, `SZCB`, `S`, `SB`, `C`, `CB`, `A`, `AB`, `MOV`, `MOVB`, `SOC`, and `SOCB`
- single-operand operations: `BLWP`, `B`, `X`, `CLR`, `NEG`, `INV`, `INC`, `INCT`, `DEC`, `DECT`, `BL`, `SWPB`, `SETO`, and `ABS`
- immediate and workspace/status operations: `LI`, `AI`, `ANDI`, `ORI`, `CI`, `STWP`, `STST`, `LWPI`, `LIMI`, `IDLE`, `RSET`, `RTWP`, `CKON`, `CKOF`, and `LREX`
- extended operation: `XOP`
- shifts: `SRA`, `SRL`, `SLA`, and `SRC`
- jumps: `JMP`, `JLT`, `JLE`, `JEQ`, `JHE`, `JGT`, `JNE`, `JNC`, `JOC`, `JNO`, `JL`, `JH`, and `JOP`
- CRU operations: `SBO`, `SBZ`, `TB`, `LDCR`, and `STCR`
- multiply and divide: `MPY` and `DIV`
- `NOP`, encoded as `JMP 0`

## Encodings and validation

General-address operands are accepted in every ISA position that specifies a general address. Their six-bit fields are `00rrrr` (register), `01rrrr` (indirect), `11rrrr` (auto-increment), and `10rrrr` (symbolic/indexed). Symbolic and indexed operands append one address word. For two-operand instructions, extension words are emitted in source-then-destination order.

`LI`, `AI`, `ANDI`, `ORI`, and `CI` take `register, word`; `STWP` and `STST` take one register; `LWPI` takes one word; and `LIMI` takes its architecturally defined 4-bit mask (`0` through `15`) in an extension word. Shift counts are the 4-bit literal field (`0` through `15`; zero selects the count in `R0` on the processor).

`XOP` takes `general-address, vector`, where `vector` is `0` through `15`; its general-address extension, when present, follows the instruction word. `MPY` and `DIV` take `source, register`. `LDCR` and `STCR` take `general-address, count`, with counts `0` through `16`; a count of `16` is encoded as the ISA's zero count field (and an explicit `0` preserves that raw encoding).

Jump operands are absolute byte addresses or labels. The assembler converts them to the signed, 8-bit word displacement from the next instruction; targets must be word-aligned and within `-128` through `127` words. `SBO`, `SBZ`, and `TB` accept signed CRU offsets from `-128` through `127`.

## Example

```asm
        lwpi >8300
        li r1, >0001
loop:
        ai r1, 1
        mov r1, @>8c00
        jmp loop
```

The generic bare target deliberately does not select TI-99/4A console ROM entry points, cartridge headers, VDP routines, or workspace conventions beyond the instructions explicitly written in the source.
