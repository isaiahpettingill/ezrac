# Game Boy Assembly

EZRA provides separate targets for the original monochrome Game Boy and Game
Boy Color:

```text
gameboy-dmg-lr35902
gameboy-color-lr35902
```

Both targets use the dedicated `lr35902` assembler and emit a valid 32 KiB
ROM-only cartridge. DMG builds use the `.gb` extension and compatibility byte
`0x00`; Game Boy Color builds use `.gbc` and compatibility byte `0xC0`. The
packager writes the entry stub, Nintendo logo, title, ROM/RAM type fields,
header checksum, and global checksum.

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

Game Boy targets can compile `.ezra` source directly. The initial LR35902 ABI
sets `SP` to `FFFEh`, calls `_main` from the cartridge entry code, and enters a
HALT loop when `main` returns. Functions currently support parameterless calls,
`return`, and LR35902 `asm` blocks. Unsupported high-level statements and
function signatures are rejected with target-specific diagnostics rather than
being lowered as incompatible Z80 code.

`embed` declarations place raw files or literal bytes in cartridge ROM and
expose `_name` and `_name_end` assembly labels. This makes preconverted 2bpp
tiles, tile maps, sprite sheets, palettes, music, and other binary assets easy
to package without `incbin`:

```ezra
embed tiles: bytes = file("assets/tiles.2bpp")
embed map: bytes = file("assets/level.map")

fn main() {
    asm volatile {
        "ld hl, _tiles"
        // Upload through an SDK routine or an LR35902 copy loop.
    }
}
```

Complete projects live under `examples/gameboy`: `serial-hello`, `background`,
and `sprite`. Source projects can import these built-in modules on both DMG and
CGB targets:

- `gb.video`: safe LCD shutdown, VRAM byte copying, background-map clearing,
  and standard BG/OBJ LCD setup.
- `gb.sprites`: blank background tile setup, OAM sprite display, and hide-all.
- `gb.serial`: zero-terminated serial output for emulator consoles and link
  diagnostics.

The initial SDK uses a documented register ABI where data-dependent operations
need it: `video.copy_bytes()` accepts source in `HL`, destination in `DE`, and
byte count in `B`; `serial.send_cstr()` accepts the string address in `HL`.
Thus applications generally need only a short asset-address setup block while
all hardware loops and synchronization stay in reusable SDK functions. Typed
SDK parameters will replace these final register loads as high-level LR35902
argument lowering expands.

The backend currently emits 32 KiB ROM-only cartridges; mapper banking,
high-level expression lowering, and interrupt functions remain future
extensions.

## Macro SDK

Vendor `toolchains/gameboy-lr35902/sdk/asm/gb` into the project. Include
`hardware.inc` for DMG/common programming or `color.inc` for CGB additions.
The SDK covers hardware registers and common idioms for interrupt vectors,
LCD/VRAM access, OAM DMA, joypad polling, timers, serial, audio channels,
memory copy/fill, MBC banking, CGB VRAM/WRAM banking, palettes, HDMA, and speed
switching.

The macros deliberately do not hide hardware rules: access VRAM and palette
RAM only in safe PPU modes, run portable OAM DMA code from HRAM with interrupts
controlled, switch ROM banks from fixed ROM, and disable LCD only in VBlank.

Primary references:

- Pan Docs: <https://gbdev.io/pandocs/>
- Complete opcode table: <https://gbdev.io/gb-opcodes/optables/>
- RGBDS instruction reference: <https://rgbds.gbdev.io/docs/v1.0.1/gbz80.7>
