# TMS9900 Assembly

The optional `tms9900` feature enables the standalone TMS9900 assembler, generic `bare-tms9900` raw-binary target, and the `ti99-4a-tms9900` source target. The TI-99/4A target emits a one-bank cartridge ROM beginning at `>6000`, including the standard cartridge header and an `EZRA` menu entry. It includes the embedded `ti99.console`, `ti99.graphics`, `ti99.input`, `ti99.memory`, `ti99.sound`, `ti99.sprites`, and `ti99.vdp` SDK modules.

The initial source ABI evaluates scalar values in `R0`, uses `R1`/`R2` as scratch registers, and keeps arguments and call links in compiler-owned expansion RAM. Calls also place their first ten arguments in `R0` through `R9`, giving naked platform wrappers a stable register ABI. Recursive functions are not supported. Source code currently supports 8- and 16-bit scalar variables, calls, basic arithmetic/bitwise operations, comparisons, loops, MMIO, and inline assembly; arrays, structs, 24-bit values, shifts, multiplication, division, and remainder remain unsupported by this backend.

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
- workspace-register operations: `COC`, `CZC`, and `XOR`; multiply and divide: `MPY` and `DIV`; extended-operation dispatch: `XOP`
- pseudo instructions: `NOP` and `RT`

`NOP` is accepted as the `JMP 0` encoding and `RT` as `B *R11`. Shift operands follow TI syntax: `SRA R6, 4` (register then count). Jump targets must be word-aligned and fit the TMS9900 signed 8-bit word displacement range. `SBO`, `SBZ`, and `TB` accept signed CRU offsets from `-128` through `127`. `dw` data words use the TMS9900's big-endian byte order.

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

## TI-99/4A source target

```sh
cargo run --features tms9900 -- build --target ti99-4a-tms9900 examples/ti99-4a/hello.ezra
```

```ezra
import ti99.vdp

fn main() {
    vdp.write_data('E')
}
```

The emitted `.bin` is a raw 8 KiB-bank-compatible cartridge ROM. It assumes conventional 32 KiB expansion RAM at `>A000..>CFFF`; console scratchpad RAM at `>8300..>83FF` is reserved for the compiler workspace and hardware ABI.

### VDP and sprite primitives

`ti99.vdp` provides `init_graphics`, `set_register`, `set_write_address`, `set_read_address`, `write_data`, `write_bytes`, `fill`, `clear_name_table`, `init_sprites`, `set_sprite`, `hide_sprites`, and `wait_frames`. `ti99.graphics` adds Graphics I setup, color-table, pattern-upload, VRAM-fill, and tile-write helpers; `ti99.sprites` adds `enable`, `enable_graphics`, sprite-pattern upload, placement, hiding, and timing helpers.

```ezra
import ti99.graphics

fn main() {
    graphics.begin()
    graphics.set_color_byte(0xF4)
}
```

The Mandelbrot and atom examples use these primitives for VDP setup, colors, sprite descriptors, and timing; their remaining inline assembly is limited to ROM-resident tile/sprite data and target-specific math.
