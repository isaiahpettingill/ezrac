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

The assembler supports the following instruction families:

- two-operand word and byte operations: `SZC`, `SZCB`, `S`, `SB`, `C`, `CB`, `A`, `AB`, `MOV`, `MOVB`, `SOC`, and `SOCB`
- single-operand operations: `BLWP`, `B`, `X`, `CLR`, `NEG`, `INV`, `INC`, `INCT`, `DEC`, `DECT`, `BL`, `SWPB`, `SETO`, and `ABS`
- immediate and workspace/status operations: `LI`, `AI`, `ANDI`, `ORI`, `CI`, `STWP`, `STST`, `LWPI`, `LIMI`, `IDLE`, `RSET`, `RTWP`, `CKON`, `CKOF`, and `LREX`
- shifts: `SRA`, `SRL`, `SLA`, and `SRC`
- jumps: `JMP`, `JLT`, `JLE`, `JEQ`, `JHE`, `JGT`, `JNE`, `JNC`, `JOC`, `JNO`, `JL`, `JH`, and `JOP`
- CRU operations: `SBO`, `SBZ`, `TB`, `LDCR`, and `STCR`
- multiply and divide: `MPY` and `DIV`

`NOP` is accepted as the `JMP 0` encoding. Jump targets must be word-aligned and fit the TMS9900 signed 8-bit word displacement range. `SBO`, `SBZ`, and `TB` accept signed CRU offsets from `-128` through `127`.

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
