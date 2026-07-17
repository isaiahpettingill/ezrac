# Game Boy Assembly

EZRA provides separate targets for the original monochrome Game Boy and Game
Boy Color:

```text
gameboy-dmg-lr35902
gameboy-color-lr35902
```

Both targets use the dedicated `lr35902` assembler. DMG builds use the `.gb`
extension and compatibility byte `0x00`; Game Boy Color builds use `.gbc` and
compatibility byte `0xC0`. The packager writes the entry stub, Nintendo logo,
title, ROM/RAM type fields, header checksum, and global checksum.

Without a `[gameboy]` mapper configuration, builds emit a valid 32 KiB ROM-only
cartridge. `[gameboy]` can instead produce an MBC1 or MBC5 ROM-banked cartridge
as configured below.

## Project

```toml
[project]
name = "serial-hello"

[build]
input = "src/main.asm"
input_kind = "assembly"
target = "gameboy-dmg-lr35902"
output = "gb"
```

Code starts at `0x0150`:

```asm
    di
    ld sp, 0FFFEh
    ld hl, message
.loop:
    ld a, (hl+)
    and a
    jr z, .halt
    ldh (01h), a
    ld a, 81h
    ldh (02h), a
.wait:
    ldh a, (02h)
    and 80h
    jr nz, .wait
    jr .loop
.halt:
    halt
    jr .halt
message:
    db "Hello", 0
```

## ROM Banking

Use `[gameboy]` to request a mapper cartridge and append raw data banks to the
ROM:

```toml
[gameboy]
mapper = "mbc5"              # "mbc1" or "mbc5"
rom_banks = 8                 # optional power of two: 2 through 512
ram_banks = 4                # optional: 0, 1, 4, 8, or 16
battery = true               # optional; requires ram_banks > 0
rumble = false               # optional; MBC5 only
bank_files = [
    "assets/bank2.bin",
    "assets/bank3.bin",
]
```

`bank_files` are raw ROM data, not separately assembled or linked programs.
The compiler keeps executable code in fixed ROM bank 0 and the initial
switchable ROM bank 1. The first file occupies switchable bank 2, with each
later file assigned to the next selectable switchable bank. A file may be at
most 16 KiB (`0x4000` bytes); code selects its bank and reads it through the
`0x4000..0x7FFF` ROM window.

### Source-declared banked embeds

Enable explicit source banking only for a Game Boy target with an MBC mapper:

```toml
[banking]
enabled = true

[gameboy]
mapper = "mbc1" # or "mbc5"; `rom-only` is rejected
```

`@cfg(bank(N))` can place `embed`, `global`, and `fn` declarations in exactly
ROM bank `N`. Banked embeds and globals receive addresses in the switchable
`0x4000..0x7FFF` window; globals are ROM-resident and therefore read-only.
Banked functions are called through generated resident bank-0 far-call support:
it saves the active bank, maps the target bank, invokes the function, and
restores the caller's bank. Nested far calls are safe.

`ptr@N` documents an access through bank `N`. It is accepted in a bank-0 helper
(including an ordinary unbanked function), where the program must map `N`
manually before dereferencing it, or inside a function declared for that same
bank. The qualifier does not itself switch the mapper.

```ezra
@cfg(bank(2))
embed level: bytes = bytes [0xA1, 0xB2]

fn read_level() -> u8 {
    // Select bank 2 first, then access the bank-2 pointer.
    asm volatile { "ld a, 2" "call __ezra_gb_select_bank" }
    return *(level.ptr@2)
}
```

The generated `__ezra_gb_select_bank` helper is always in ROM bank 0. It
selects the low eight-bit bank value in `A`; far-call support additionally uses
its internal 9-bit entry for MBC5. It programs both MBC1 ROM-bank fields, so
selectable MBC1 banks above 31 work correctly. The runtime does **not**
automatically select a bank for `ptr@N`.

Banked globals require compile-time initializers and are read-only because they
live in cartridge ROM. Banked embeds cannot use `align`. Explicit source-banked
content cannot share a bank with an automatically assigned
`gameboy.bank_files` payload. See `examples/gameboy/banking` for a complete
MBC5 project that far-calls banked code and displays banked tile data.

`ram_banks` describes 8 KiB external RAM banks. Omit it, or set it to `0`, for
no cartridge RAM. `battery = true` requests battery-backed cartridge RAM and
therefore requires a nonzero `ram_banks` value. `rumble = true` selects an
MBC5 rumble cartridge; it is invalid with MBC1. On an MBC5 rumble cartridge,
the high RAM-bank-select bit is the rumble motor control, so `ram_banks = 16`
is not available; use at most 8 RAM banks.

MBC1 has additional hardware restrictions:

- It supports at most four external RAM banks, so `ram_banks = 8` and `16`
  require MBC5.
- Its ROM bank register cannot select banks `0x00`, `0x20`, `0x40`, or
  `0x60`; a zero low five-bit field aliases to the next bank. EZRA skips those
  non-selectable physical slots when assigning bank files (after bank `0x1F`,
  the next payload is placed in bank `0x21`). Standard MBC1 therefore provides
  at most 123 bank-file slots after the code banks.
- MBC1 RAM-banking mode also changes the `0x0000..0x3FFF` mapping. Change RAM
  banks from a routine executing in the `0x4000..0x7FFF` window whose ROM bank
  remains selected (or from a deliberately mirrored trampoline), and do not
  return to a remapped lower-ROM address.

MBC5 has a 9-bit ROM-bank number and can select banks `0x000..0x1FF`; with
banks 0 and 1 reserved for generated code, it provides up to 510 bank-file
slots. EZRA far calls support the ninth bit. The MBC macros in `hardware.inc`
take register-field values rather than trying to infer a full bank number: use
the MBC1 low/high fields or the MBC5 low-byte/high-bit pair.

External RAM is unavailable until enabled and should be disabled when it is not
being accessed. Mapper writes target cartridge address ranges rather than I/O
registers. Run ordinary ROM-bank switching code from fixed ROM bank 0: changing
the currently executing `0x4000..0x7FFF` bank changes subsequent instruction
fetches immediately. The MBC1 RAM-mode caveat above is stricter.

## Instruction Set

The assembler covers all 244 executable base opcodes and all 256 CB-prefixed
opcodes documented for the SM83/LR35902. The `0xCB` base byte is emitted as a
prefix, and the 11 invalid lock-up opcodes are never emitted. Memory operands
use parentheses. Game Boy-specific forms include:

```asm
ld a, (hl+)
ld (hl-), a
ldh (80h), a
ldh a, (c)
ld hl, sp-4
add sp, -4
swap (hl)
stop
```

Z80-only instructions and registers such as `in`, `out`, `exx`, `djnz`, `ix`,
`iy`, `i`, `r`, alternate registers, indexed operands, and ED-family block
instructions are rejected. LR35902 conditional branches support only `nz`,
`z`, `nc`, and `c`. Relative branches must fit `-128..127`; absolute addresses
are 16-bit and encoded little-endian. `stop` emits the required two-byte
`10h 00h` form.

The assembler accepts these Game Boy aliases:

| Canonical form | Accepted aliases |
| --- | --- |
| `jp hl` | `jp (hl)` |
| `ld (hl+), a` / `ld a, (hl+)` | `(hli)`, plus `ldi (hl), a` / `ldi a, (hl)` |
| `ld (hl-), a` / `ld a, (hl-)` | `(hld)`, plus `ldd (hl), a` / `ldd a, (hl)` |
| `ldh (n), a` / `ldh a, (n)` | `n` may be an 8-bit offset or an address in `FF00h..FFFFh` |
| `ldh (c), a` / `ldh a, (c)` | `ld (c), a` / `ld a, (c)` |

Memory operands use parentheses; RGBDS square-bracket syntax is not accepted.
Numeric operands use decimal, `0x`-prefixed hexadecimal, or `h`-suffixed
hexadecimal notation. Signed SP-relative operands use `+n` or `-n` and must fit
`-128..127`.

## EZRA Source Projects

Game Boy targets compile `.ezra` source through HIR, TBIR, and the shared
semantic storage model before LR35902 lowering. The runtime sets `SP` to
`FFFEh`, calls `_main` from the cartridge entry code, and enters a HALT loop
when `main` returns. The source ABI uses WRAM storage for scalar values,
arguments, return values, globals, strings, and embedded bytes; generated
`_name equ` symbols keep globals and embeds available to inline assembly.

The backend supports scalar operators and casts, locals/globals, pointers,
arrays, structs, strings and embeds, control flow, calls (including recursion),
inline-assembly operands, and `interrupt`/`naked` functions. It lowers to
LR35902-only instructions and validates the generated assembly with the
LR35902 encoder; unsupported target-specific operations, such as separate port
I/O, produce diagnostics.

`embed` declarations place raw files or literal bytes in cartridge ROM. Project
`[assets]` rules can override section and alignment per target, so the same
source declarations also work with ZX Spectrum, Agon, and custom layouts. This
makes preconverted 2bpp tiles, tile maps, sprite sheets, palettes, wave tables,
music, and other binary assets easy to package without `incbin`:

```ezra
embed tiles: bytes = file("assets/tiles.2bpp")
embed map: bytes = file("assets/level.map")

import gb.video
import gb.sprites

fn main() {
    video.begin_update()
    video.copy_bytes(&tiles, 0x8000, 16)
    sprites.upload_tile1(&tiles)
}
```

Complete projects live under `examples/gameboy`: `serial-hello`, `background`,
`sprite`, `input-audio`, and `color-input`. Source projects can import these built-in modules
on both DMG and CGB targets:

- `gb.video`: safe LCD shutdown, VRAM byte copying, background-map clearing,
  standard BG/OBJ LCD setup, and SCX scroll helpers.
- `gb.sprites`: blank background tile setup, OAM sprite display, and hide-all.
- `gb.serial`: zero-terminated serial output for emulator consoles and link
  diagnostics.
- `gb.input`: normalized active-high reads for Right, Left, Up, Down, A, B,
  Select, and Start, plus per-control wait helpers.
- `gb.color`: CGB RGB555 background and sprite palette uploads from embedded
  8-byte palette assets.
- `gb.audio`: APU setup, pulse beep playback, embedded 32-sample wave loading,
  and wave-channel playback.

Embedded assets can now be passed directly to SDK calls such as
`sprites.upload_tile1(&player)` and `audio.load_wave(&wave)`; applications do
not need inline assembly for ordinary sprite textures, tile uploads, serial
strings, button waits, CGB palette uploads, or wave-table audio.

The backend currently leaves high-level expression lowering and interrupt
functions as future extensions. ROM banking uses the mapper configuration
above; bank files are raw data and executable code remains in banks 0 and 1.

## Macro SDK

Vendor `toolchains/gameboy-lr35902/sdk/asm/gb` into the project. Include
`hardware.inc` for DMG/common programming or `color.inc` for CGB additions.
The SDK covers hardware registers and common idioms for interrupt vectors,
LCD/VRAM access, OAM DMA, joypad polling, timers, serial, audio channels,
memory copy/fill, MBC banking, CGB VRAM/WRAM banking, palettes, HDMA, and speed
switching. For the supported mapper cartridges, use `GB_MBC1_SELECT_ROM_BANK`
with low five-bit and high two-bit fields, or `GB_MBC5_SELECT_ROM_BANK` with
low-byte and high-bit fields. Use the matching RAM-bank macro, and pair RAM
access with `GB_MBC_ENABLE_RAM` and `GB_MBC_DISABLE_RAM`.

The macros deliberately do not hide hardware rules: access VRAM and palette
RAM only in safe PPU modes, run portable OAM DMA code from HRAM with interrupts
controlled, switch ROM banks from fixed ROM, and disable LCD only in VBlank.
MBC1 RAM-mode switching has the lower-ROM remapping caveat described above;
MBC5 rumble uses `GB_MBC5_SELECT_RUMBLE_RAM_BANK` so its motor bit is not
mistaken for a fourth RAM-bank bit.

Primary references:

- Pan Docs: <https://gbdev.io/pandocs/>
- Complete opcode table: <https://gbdev.io/gb-opcodes/optables/>
- RGBDS instruction reference: <https://rgbds.gbdev.io/docs/v1.0.1/gbz80.7>
