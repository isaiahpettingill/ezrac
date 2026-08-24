# Targets and layouts

## Target triples

Targets use a `vendor-platform-cpu[-version]`-style name. The compiler finds a CPU family in the components and then applies a target-specific layout and package rule when one exists. Run:

```sh
ezrac targets
```

The current source-oriented targets include:

| Target | CPU/address width | Output | Notes |
| --- | --- | --- | --- |
| `agonlight-mos-ez80` | eZ80 ADL / 24-bit | `.bin` | Main Agon MOS source target; built-in `agon.*` SDK. |
| `custom-unknown-ez80` | eZ80 ADL / 24-bit | `.bin` | Generic eZ80 source target. |
| `ezra-test-flat-ez80` and `ezra-test-split-ez80` | eZ80 ADL / 24-bit | `.bin` | VM test harness profiles. |
| `cpm-2.2-z80` | Z80 / 16-bit | `.com` | CP/M source and assembly target. |
| `zxspectrum-z80` | Z80 / 16-bit | `.tap` | Experimental Spectrum target. |
| `gameboy-dmg-lr35902` | LR35902 / 16-bit | `.gb` | DMG source and assembly target. |
| `gameboy-color-lr35902` | LR35902 / 16-bit | `.gbc` | CGB source and assembly target. |
| `msdos-com-i8086` | 8086 / 16-bit segment | `.com` | DOS `.COM` startup and `dos.*` SDK. |
| `nes-2a03` | 2A03 / 16-bit | `.nes` | Super-alpha source and raw assembly target. |
| `snes-5a22` | 5A22/65C816 / 24-bit | `.sfc` | Super-alpha 32 KiB LoROM package. |
| `generic-pic18-bare` | PIC18 / 21-bit program | `.hex` | Source and classic PIC18 assembly. |
| `bare-avr` and family targets | AVR / 16-bit | `.bin` or `.hex` | Optional `avr` backend. |
| `generic-dcpu-bare` | DCPU-16 / 16-bit | `.bin` | Optional `dcpu` backend. |
| `generic-m68k-bare` | 68000 / 24-bit | `.bin` | Optional `m68k` experimental backend. |
| `ti99-4a-tms9900` | TMS9900 / 16-bit | `.bin` | Optional `tms9900` cartridge target. |

This table is a guide, not a guarantee that all targets have the same source-language coverage. The detailed [platform guide](platforms.md) gives support tiers and current limitations.

## Default layouts

A layout maps logical output sections to address ranges. It also defines load and entry addresses, stack placement, and symbols consumed by startup code or SDKs. Inspect the selected default with:

```sh
ezrac layout
```

## `.ezralayout` syntax

```text
layout demo {
    load 0x010000
    entry 0x010040
    stack 0x0FFF00

    region code 0x010000..0x03FFFF read execute
    region rodata 0x040000..0x04FFFF read
    region ram 0x050000..0x0BFFFF read write
    region stack 0x0F0000..0x0FFFFF read write reserved

    section .header -> code align 1
    section .text -> code align 16
    section .rodata -> rodata align 16
    section .data -> ram align 16
    section .bss -> ram align 1
    section .assets -> rodata align 1
    section .scratch -> ram align 1

    symbol EZRA_LOAD_ADDR = 0x010000
    symbol EZRA_ENTRY_ADDR = 0x010040
    symbol EZRA_STACK_TOP = 0x0FFF00
}
```

Regions have `read`, `write`, `execute`, `volatile`, and `reserved` flags. Addresses are checked against the selected target address width. Sections are placed in regions with the requested alignment. `load`, `entry`, and `stack` are the image load address, program entry point, and stack top.

Use a layout for any build-like command:

```sh
ezrac layout layouts/custom.ezralayout
ezrac build --layout layouts/custom.ezralayout src/main.ezra
ezrac assemble --layout layouts/custom.ezralayout src/main.asm
```

The layout also controls where `.text`, `.rodata`, `.data`, `.bss`, `.assets`, and custom sections fit. If assembled bytes do not fit their assigned region, the compiler reports an error instead of silently producing an invalid image.

## Output formats

The common formats are raw `bin`, CP/M/DOS `com`, Intel HEX `hex`, ZX Spectrum `tap`, Game Boy `gb`, C64 `prg`, TI `8xp`, MSP430 `elf`, Arduboy `arduboy`, and target-specific cartridge packages. The target may add headers, checksums, vectors, loaders, or fixed-size padding. See [compiler usage](usage.md#output-formats) for the complete format list.

The Agon MOS layout puts a MOS header at `0x040000`, code at `0x040045`, read-only data at `0x060000`, RAM at `0x070000`, assets at `0x0C0000`, and a reserved stack window near the top of the 24-bit address space. Normal Agon programs return from `main` to MOS.
