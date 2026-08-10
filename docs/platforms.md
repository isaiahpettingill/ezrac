# EZRA Platform Documentation

EZRA targets are selected with target triples. A target triple has this general shape:

```text
vendor-platform-cpu[-version]
```

The compiler identifies the CPU by scanning target components for a supported CPU family, including eZ80/Z80/R800 variants, Intel 8080/8085/8086, LR35902, MOS 6502, WDC 65C816, TMS9900, DCPU-16, AVR, M6800/M6809, and M68k. Default builds enable every compiler backend. Consumers using `--no-default-features` can enable only the backend features they need; every backend supports `no_std + alloc` compilation.

Only CPUs with an implemented memory model can be resolved. A resolvable target does not necessarily have complete EZRA source code generation; optional DCPU-16, M6800, M68k, and TMS9900 targets have target-specific source emitters. MOS 6502 variants also have a source emitter; the initial TMS9900 backend is a documented 8-/16-bit scalar subset. AVR has a complete register-ABI source backend.

## Support Levels

Platform support is tiered by its strongest published evidence:

| Tier | Meaning |
| --- | --- |
| **Tier 1** | Examples are built and behaviorally verified on a real third-party libretro core. |
| **Tier 2** | Source behavior is covered by automated compiler, VM/emulator, SDK, and packaging tests, but not a published real-core run. |
| **Tier 3** | Build, assembly, or packaging paths are tested; runtime behavior on target hardware or a real core is not yet published. |
| **Tier 4** | Target profile or SDK scaffolding exists, but substantial backend or runtime validation remains. |

Tier 1 is not a claim that every program or hardware feature works. It means the repository's representative examples passed the assertions listed in the [published real-core results](real-core-test-results.md). The current production-quality source path remains eZ80-oriented; use `assemble` when exact machine control is required.

**Super alpha** marks a target whose output works for current examples but whose compiler ABI, package format, and SDK API may still change without compatibility support. NES, SNES, and Commodore 64 currently have this status regardless of their test tier.

## Target Summary

| Target pattern | Tier | CPU | Address width | Default output | Built-in SDK | Status |
| --- | ---: | --- | ---: | --- | --- | --- |
| `agonlight-mos-ez80` | 2 | eZ80 ADL | 24 | Agon MOS `.bin` | `agon.*` | Main source target; real-core publication pending |
| `custom-unknown-ez80` | 2 | eZ80 ADL | 24 | `.bin` | none | Generic eZ80 source target |
| `ez180n-ez80` | 1 | eZ80 ADL | 24 | `.gaem` | `ez180n.*` | Four examples verified on ez180N nightly |
| `ezra-test-flat-ez80` | 2 | eZ80 ADL | 24 | `.bin` | `harness.*` | Automated test harness target |
| `ezra-test-split-ez80` | 2 | eZ80 ADL | 24 | `.bin` | `harness.*` | Automated test harness target |
| `ti84plusce-ez80` | 2 | eZ80 ADL | 24 | `.8xp` | `tice.*` | Protected programs and 16-bit graphics verified on CEmu |
| `ti83premiumce-ez80` | 3 | eZ80 ADL | 24 | `.8xp` | `tice.*` | Shares the CEmu-verified CE runtime; model-specific validation pending |
| `zxspectrum-z80` | 1 | Z80 | 16 | `.tap` | `zx.*` | Hello example verified on Fuse |
| `gameboy-dmg-lr35902` | 1 | LR35902 | 16 | `.gb` | `gb.*` | Four DMG examples verified on mGBA |
| `gameboy-color-lr35902` | 1 | LR35902 | 16 | `.gbc` | `gb.*` | CGB input example verified on mGBA |
| `ti83-z80` | 3 | Z80 | 16 | `.8xp` | `ti.*` | Experimental TI Z80 target |
| `ti83plus-z80` | 3 | Z80 | 16 | `.8xp` | `ti.*` | Experimental TI Z80 target |
| `ti84-z80` | 3 | Z80 | 16 | `.8xp` | `ti.*` | Experimental TI Z80 target |
| `ti84plus-z80` | 3 | Z80 | 16 | `.8xp` | `ti.*` | Experimental TI Z80 target |
| `cpm-2.2-z80` | 1 | Z80 | 16 | `.com` | `cpm.*` | Seven source/assembly examples verified on ep128emu IS-DOS |
| `cpm-*-i8080` | 4 | 8080 | 16 | `.com` | `cpm.*` | Assembly/source scaffold |
| `cpm-*-i8085` | 4 | 8085 | 16 | `.com` | `cpm.*` | Assembly/source scaffold |
| `bare-z80` | 4 | Z80 | 16 | `.bin` | none | Bare assembly/source scaffold |
| `bare-r800` | 4 | R800 | 16 | `.bin` | none | Bare assembly/source target with built-in VM execution |
| `bare-z80n` | 4 | Z80N | 16 | `.bin` | none | Bare assembly/source scaffold |
| `bare-z180` | 4 | Z180 | 16 | `.bin` | none | Bare assembly/source scaffold |
| `bare-i8080` | 4 | 8080 | 16 | `.bin` | none | Bare assembly/source scaffold |
| `bare-i8085` | 4 | 8085 | 16 | `.bin` | none | Bare assembly/source scaffold |
| `bare-i8086` | 3 | 8086 | 16 (single segment) | `.bin` | none | Optional `i8086` feature; generic source backend and complete strict assembler |
| `msdos-com-i8086` | 3 | 8086 | 16 (single segment) | `.com` | `dos.*` | First-class DOS `.COM` startup, packaging, and SDK; emulator validation pending |
| `bare-ez80` | 3 | eZ80 ADL | 24 | `.bin` | none | Bare eZ80 target |
| `commodore64-6502` | 2 | MOS 6510 (6502-compatible) | 16 | `.prg` | `c64.*` | **Super alpha**; optional `mos6502` feature; source and assembly target |
| `generic-6502-bare` | 3 | MOS 6502 | 16 | `.bin` | none | Optional `mos6502` feature; bare source/assembly target |
| `generic-65c816-bare` | 3 | WDC 65C816 | 24 | `.bin` | none | Native-mode byte ABI, 24-bit pointers, and bank-00 near calls |
| `nes-2a03` | 3 | Ricoh 2A03 | 16 | `.nes` | `nes.*` | **Super alpha**; optional `mos6502` feature; EZRA source, bundled SDK, and raw assembly NROM-128 target |
| `snes-5a22` | 3 | Ricoh 5A22 / 65C816 core | 24 | `.sfc` | `snes.*` | **Super alpha**; native source and assembly, 32 KiB LoROM packaging, generated header/checksum/vectors |
| `sega-master-system-z80` | 3 | Z80 | 16 | `.sms` | `sms.*` | 32-256 KiB export-SMS target with slot-2 data banking; interrupt runtime pending |
| `sega-game-gear-z80` | 3 | Z80 | 16 | `.gg` | `sms.*`, `gg.*` | 32-256 KiB Game Gear target with shared slot-2 data banking and Game Gear hardware helpers |
| `ti99-4a-tms9900` | 3 | TMS9900 | 16 | cartridge `.bin` | `ti99.*` | Optional `tms9900` feature; TI-99/4A scalar source/assembly target |
| `bare-tms9900` | 3 | TMS9900 | 16 | `.bin` | none | Optional `tms9900` feature; bare scalar source/assembly target |
| `msp430-none-elf` | 3 | MSP430 | 16 | ELF32 `.elf` | none | Optional `msp430` feature; little-endian ELF32 source/assembly target |
| `msp430x-none-elf` | 3 | MSP430X | 20 | ELF32 `.elf` | none | Optional `msp430` feature; extended-address ELF32 source/assembly target |
| `generic-dcpu-bare` | 3 | DCPU-16 | 16 | `.bin` | `dcpu.*` | Optional `dcpu` feature; complete handwritten assembly and limited scalar source backend |
| `bare-avr` | 3 | AVR | 16 | `.bin` | none | Optional `avr` feature; register-ABI source/assembly target |
| `arduboy-avr` | 3 | AVR | 16 | Intel HEX `.hex` | `arduboy.*` | Optional `avr` feature; ATmega32U4 source/assembly target |
| `generic-m68k-bare` | 3 | Motorola 68000 | 24 | `.bin` | none | Optional `m68k` feature; experimental scalar source/assembly target |

Any triple containing a supported CPU can resolve if its CPU has a memory model. Unknown platform names usually fall back to a generic layout for that CPU unless they match a special layout rule.

## Bare Intel 8086

Enable the `i8086` feature to build EZRA source or assemble a raw, single-segment 8086 binary:

```sh
cargo run --features i8086 -- build --target bare-i8086 program.ezra
```

```sh
cargo run --features i8086 -- assemble --cpu i8086 --target bare-i8086 --base 100h -o program.bin program.asm
```

`build` writes the generated `.asm`, `.map`, and `.bin` files under `target/bare-i8086/`; output-path overrides are available on `assemble`, not `build`.

The 8086 hardware has a 20-bit physical address bus, while the initial bare profile intentionally exposes one 16-bit, 64 KiB segment. Other resolvable i8086 triples fall back to this CPU-appropriate generic layout instead of the 24-bit eZ80 layout. The source backend covers scalar values and recursion, pointers, aggregate storage, control flow, memory and port I/O, constrained interrupt handlers, and typed inline assembly through HIR/TBIR; aggregate parameters and returns are rejected in favor of pass-by-pointer APIs. The assembler covers the complete documented 8086 opcode/form set and strictly rejects 80186/80286 and undocumented additions. CLI and library source compilation strictly validate generated assembly and require its assembled `.text` bytes to fit the selected layout's `.text` region. See [`i8086-assembly.md`](i8086-assembly.md).

## MS-DOS `.COM` on Intel 8086

Use `msdos-com-i8086` for a flat `.COM` program with a PSP at `0000h..00FFh` and code loaded and entered at `0100h`:

```sh
cargo run --features i8086 -- build --target msdos-com-i8086 program.ezra
```

Startup establishes `DS=ES=CS`, verifies DOS granted at least `0F80h` paragraphs for the fixed layout and a 2 KiB minimum stack area, preserves the loader-provided `SS:SP`, clears the direction flag, calls `main`, and terminates a returning program with `INT 21h/AH=4Ch`. The built-in `dos.*` SDK covers console, handle-based files, directories and DTA searches, memory allocation and segment-aware access, date/time, process services, errors, constants, and PSP/FCB/JFT/environment helpers. DOS 2.0 is the baseline; newer wrappers are marked DOS 3+/4+/5+, and `EXEC` is DOS 3+ because DOS 2.x may destroy `SS:SP`.

Pointers remain 16-bit near offsets. Segment values returned by DOS allocation are not pointers; use `dos.memory` far-memory accessors. Copy the PSP command tail before default-DTA searches or full default-FCB operations because these areas overlap. MZ `.EXE` packaging and deterministic emulator-backed runtime tests remain deferred. See [`msdos-sdk.md`](msdos-sdk.md) and `examples/msdos-i8086/`.

## Generic DCPU-16

Enable the `dcpu` feature to build a raw little-endian DCPU-16 image:

```sh
cargo run --features dcpu -- build --target generic-dcpu-bare src/main.ezra
cargo run --features dcpu -- build examples/dcpu-16/lem-hello/main.asm
```

The target has a complete DCPU-16 1.7 handwritten assembler, a vendorable
standard-machine macro SDK, and a limited scalar source backend. Images are
compatible with the Standard Compatibility profile of
[`dcpu-16-libretro`](https://github.com/isaiahpettingill/dcpu-16-libretro), which
maps LEM1802 screen memory at word address `0x8000`. See
[`examples/dcpu-16`](../examples/dcpu-16/) and
[`dcpu-assembly.md`](dcpu-assembly.md).

## AVR and Arduboy

Build bare AVR source or the Arduboy ATmega32U4 target with the `avr` feature:

```sh
cargo run --features avr -- build --target bare-avr src/main.ezra
cargo run --features avr -- build --target arduboy-avr src/main.ezra
```

`bare-avr` produces a raw flash image. `arduboy-avr` produces Intel HEX by default and reserves the upper 4 KiB of the ATmega32U4's 32 KiB flash for the Caterina bootloader. Set `[build].output = "arduboy"` and provide the required `[arduboy]` `title`, `author`, and `version` fields to instead create a schema-v2 `.arduboy` ZIP package containing `info.json` and `<executable>.hex`. Optional metadata fields are `description`, `date`, `genre`, and `source_url`. Both initialize the hardware stack to `0x0AFF`, clear `r1`, and call `main` from the reset entry.

The AVR backend lowers scalar values, pointers, arrays, structs, strings, embedded data, control flow, calls, interrupts, and inline assembly through HIR, TBIR, and the target semantic model. Its register ABI starts argument slots in `r24`, `r22`, `r20`, and `r18`, uses adjacent registers for values that fit without overlapping an earlier slot, and returns values in `r24` through `r26`. The compiler passes remaining or overlapping source-level arguments through call-marshalling storage, so source functions are not limited to four parameters. Assembly routines that follow the register ABI must use only its non-overlapping register arguments. The bundled `arduboy.core`, `arduboy.input`, and `arduboy.oled` modules use this ABI; see `examples/arduboy/snake` for a playable example.

The backend allocates source-visible storage in the AVR data address space and emits initialization code for globals, strings, and embedded data. AVR builds are validated through lowering, exhaustive instruction encoding, assembly, and Intel HEX packaging tests.

## Generic M68k

Target:

```text
generic-m68k-bare
```

Build EZRA source with the optional backend enabled:

```sh
cargo run --features m68k -- build --target generic-m68k-bare src/main.ezra
```

The generic target emits a raw big-endian 68000 image in a 24-bit address space, initializes `SP` from the target layout, calls `main`, then loops. EZRA scalar values and returns use `D0`; `D1` and `D2` are compiler scratch registers. Calls pass arguments through compiler-owned static slots, so recursive calls are not supported yet. There is no platform SDK, Sega Genesis packaging, or published runtime validation.

## Agon Light MOS

Target:

```text
agonlight-mos-ez80
```

Use this target for normal Agon MOS executables.

```toml
[build]
target = "agonlight-mos-ez80"
output = "bin"
```

The generated binary uses the Agon MOS executable shape:

```text
byte 0       JP 0x040045
byte 64      "MOS", 0, 1
byte 69      compiled program code
entry        0x040045
```

Default layout highlights:

```text
0x040000..0x04003F   MOS executable header
0x040045..0x05FFFF   code
0x060000..0x06FFFF   rodata
0x070000..0x0BFFFF   RAM
0x0C0000..0x0DFFFF   assets
0x0E0000..0x0EFFFF   VDP volatile area
0x0F8000..0x0FFFFF   reserved stack window
```

Built-in SDK modules:

```text
agon.mos
agon.console
agon.vdp
agon.buffers
agon.sprites
agon.keyboard
agon.mouse
agon.gpio
```

Coding guidance:

```ezra
import agon.console

fn main() {
    console.print_line("Hello, Agon")
}
```

Run `ezrac targets` to print this repository's documented target triples, default output extensions, SDK module families, and support status.

Let `main` return to MOS for normal programs. Emulator automation helpers exist in the SDK, but user-facing MOS programs should not exit through emulator-only ports.

Use the SDK for MOS/VDP calls instead of hard-coding call sequences unless you are intentionally writing platform assembly. Keep hardware access in small wrapper functions so it can be replaced if the Agon ABI support changes.

## ez180N Libretro Console

Target:

```text
ez180n-ez80
```

The `ez180n-ez80` target emits raw `.gaem` entry-code files that load directly at `0x010000` in the ez180N libretro core. The default memory map reserves `0x00FFC0..0x00FFFF` for console metadata, but those reserved bytes are not prefixed to the raw `.gaem` file. The console exposes an `80x56` character framebuffer at `0x080000`.

Built-in SDK modules:

```text
ez180n.console
```

Coding guidance:

```ezra
import ez180n.console

fn main() {
    console.fill(console.CHAR_SPACE)
    console.put_char(76, 52, 'E')
    console.put_char(77, 52, 'Z')
    console.present()
}
```

Use `console.set_text_color(index)` and `console.set_background_color(index)` to select colors from the ANSI/xterm 256-color palette, then call `console.present()` after framebuffer writes. The defaults are `console.DEFAULT_TEXT_COLOR` and `console.DEFAULT_BACKGROUND_COLOR`. Use `console.set_font(font)` to select one of the 24 bundled 8x8 fonts, using the exported `FONT_*` constants. `console.FONT_APPLE_I` is a 7x8 source padded to 8x8. Use `console.play_sound(id)` for the beeper port and `console.button_down(player, button)` for joypad input. Call `console.wait_tick()` to wait for the next 60 Hz console tick before the next game update.

## Generic eZ80 And Test Targets

Default target:

```text
custom-unknown-ez80
```

This target uses the generic EZRA eZ80 cartridge-style memory map and raw `.bin` output. It is useful for compiler tests, emulator experiments, and custom loaders.

Test harness targets:

```text
ezra-test-flat-ez80
ezra-test-split-ez80
```

Built-in SDK modules for test targets:

```text
harness.io
harness.layout
harness.memory
```

Coding guidance:

Use `@cfg(target("ezra-test-flat-ez80"))` or memory-width predicates when writing tests that depend on exact addresses. Avoid depending on the generic map for deployed hardware; define a custom `.ezralayout` instead.

## TI CE eZ80 Calculators

Target patterns:

```text
ti84plusce-ez80
ti83premiumce-ez80
```

Default output is `.8xp`. Flash application `.8ek` output is not implemented; selecting it produces an explicit error rather than an invalid app file.

Built-in SDK modules:

```text
tice.os
tice.lcd
```

Default layout starts code at `0xD1A881`, uses RAM/rodata/assets in the `0xD3xxxx` range, and exposes `TICE_VRAM_BASE` at `0xD40000`.

Coding guidance:

Use `tice.os` and `tice.lcd` wrappers for OS and LCD access. The LCD SDK addresses the native 320×240 16-bit framebuffer and provides RGB565 colors, bounded pixels, fills, and rectangles. Keep names for TI outputs ASCII alphanumeric or `_`; `.8xp` variable names are uppercased and truncated to 8 bytes. See `examples/ti84plusce/graphics` for a CEmu-verified example.

## Classic TI Z80 Calculators

Target patterns:

```text
ti83-z80
ti83plus-z80
ti84-z80
ti84plus-z80
```

Default output is `.8xp`. Flash application `.8xk` output is not implemented; selecting it produces an explicit error rather than an invalid app file.

Built-in SDK modules:

```text
ti.os
ti.lcd
```

Default layout starts code at `0x9D95` and exposes `TI_PLOTSSCREEN` at `0x9340`.

Coding guidance:

Use 16-bit pointers and addresses. Do not assume `u24` is pointer-sized. Keep source code behind `@cfg(cpu("z80"))` or `@cfg(pointer_width(16))` when sharing code with eZ80 targets.

## ZX Spectrum Z80

Target:

```text
zxspectrum-z80
zxspectrum-z80-128k
```

`zxspectrum-z80` emits an auto-start `.tap` containing a BASIC loader and one CODE block for a conventional 48K load. It does not package extra tape assets, TZX, SCL, or Z80 snapshots.

Built-in SDK modules are:

```text
zx.rom
zx.text
zx.screen
zx.io
zx.keyboard
zx.sound
zx.memory
zx.interrupt
```

`zx.rom`, `zx.text`, `zx.screen`, the keyboard matrix functions, ULA border/beeper output, and interrupt helpers target stock Spectrum hardware. `zx.keyboard.read_kempston()` requires an optional Kempston-compatible joystick interface. AY functions in `zx.sound` and the `0x7FFD` paging functions in `zx.memory` require a 128K-compatible machine.

### 128K RAM pages

Use `zxspectrum-z80-128k` to package RAM-page payloads in a multi-block TAP. Its loader loads resident code at `0x8000`, pages each configured bank into `0xC000`, loads its data, restores page 0, and then calls the entry point. The generated stack is in fixed RAM at `0x6000..0x7FFF`, so a page switch cannot overwrite call frames.

```toml
[build]
target = "zxspectrum-z80-128k"

[[zxspectrum.banks]]
page = 1
file = "assets/level1.bin"
name = "LEVEL1"
```

Pages `1`, `3`, `4`, `6`, and `7` are available for declared payloads; pages `0`, `2`, and `5` remain reserved for normal runtime data, resident code, and the display/system RAM. Each payload is at most 16 KiB. Payloads load in ascending page order, regardless of their declaration order. Select the required page with `zx.memory.select_128k_bank()` before reading the `0xC000..0xFFFF` window, and restore page 0 before accessing ordinary globals, rodata, or unbanked embeds.

Default 48K layout highlights:

```text
0x0000..0x3FFF   ROM, reserved
0x4000..0x5AFF   display, volatile
0x5B00..0x7FFF   system, reserved
0x8000..0xBFFF   code
0xC000..0xCFFF   rodata
0xD000..0xDFFF   RAM
```

Symbols include `ZX_SCREEN_BASE`, `ZX_ATTR_BASE`, `ZX_ROM_PRINT_CHAR`, and `ZX_ROM_CLS`.

Coding guidance:

Use ROM and screen wrappers where possible. For raw keyboard reads, use the row-specific masks such as `keyboard.KEY_Z` with their matching row; `KEY0` through `KEY4` are only raw row bit positions. Treat display memory as volatile and keep stack/system memory clear unless your loader and custom layout say otherwise.

## Commodore 64

Target:

```text
commodore64-6502
```

This MOS 6510 (6502-compatible) target has direct EZRA-to-6502 code generation and writes a `.prg` executable. The file starts with the little-endian `$0801` BASIC load address and includes a tokenized `10 SYS2061` loader; machine code starts at `$080D`. PRG autostart loaders such as VICE can launch it directly. The `mos6502` Cargo feature is required.

Bundled modules are `c64.vic` (VIC-II graphics, screen/color RAM, raster, IRQ, sprites), `c64.sid` (three SID voices and filters), `c64.cia` (keyboard, joysticks, timers, IRQ), and `c64.memory` (6510 `$0001` ROM/I/O banking). Call `memory.map_roms_and_io()` before hardware MMIO after changing bank state. See `docs/targets/commodore64.md` for register API details and Play96 real-core validation.

Default layout reserves zero page and stack, loads code at `$080D`, uses `$4000..$7FFF` for read-only data, `$8000..$BFFF` for assets, `$C000..$CFFF` for RAM, and reserves `$D000..$DFFF` for volatile I/O.

## Nintendo Entertainment System

The `nes-2a03` target compiles EZRA source with the Ricoh 2A03 backend. Source builds are packaged as mapper 0 NROM-128 images with a 16-byte iNES header, one 16 KiB PRG-ROM bank, reset/NMI/IRQ vectors pointing to the generated startup code, and one 8 KiB CHR-ROM bank. Tile 0 is reserved as a solid fallback tile; configured `.assets` tiles are packed from tile 1. See [`examples/nes-2a03/source-hello`](../examples/nes-2a03/source-hello) and [`examples/nes-2a03/png-assets`](../examples/nes-2a03/png-assets).

Raw assembly builds can still provide a complete iNES NROM-128 image, including their own header, PRG-ROM, vectors, and CHR-ROM. The packager validates the mapper and page layout. See [`examples/nes-2a03/hello-world`](../examples/nes-2a03/hello-world).

The bundled `nes.*` SDK covers PPU control and data writes, palettes, direct OAM and DMA sprites, controller reads, basic pulse audio, vblank timing, and internal RAM clearing. See [`targets/nes.md`](targets/nes.md).

The source uses `$BFF0` as the file-coordinate load address for the iNES header and starts PRG code at `$C000`. Build it with:

```sh
cargo run -- build examples/nes-2a03/hello-world/helloWorld.asm
```

Run the resulting `target/nes-2a03/helloWorld.nes` in an NES emulator. The target currently has no emulator-backed NES test runner.

## Sega Master System

`sega-master-system-z80` builds 32-256 KiB export-SMS `.sms` ROMs with ordered 16 KiB data-bank files. The packager writes the standard header at `$7FF0`, pads unused ROM with `$FF`, and installs reset, IM 1, and NMI return stubs before source-generated code at `$0069`. Bundled `sms.*` modules provide polling VBlank, VDP, mode-4 video, palette, memory constants, and two standard-controller reads. Build [`examples/sega-master-system/source-hello`](../examples/sega-master-system/source-hello) with:

```sh
cargo run -- build examples/sega-master-system/source-hello/src/main.ezra
```

Banked executable code, frame interrupts, sprites, a higher-level input state API, a full PSG API, SRAM, and emulator-backed runtime testing are not implemented. See [`targets/sega-master-system.md`](targets/sega-master-system.md).

## Sega Game Gear

`sega-game-gear-z80` shares the SMS CPU layout and `sms.system`, `sms.vdp`, `sms.video`, `sms.memory`, and `sms.input` modules. It builds a 32-256 KiB export-Game-Gear `.gg` ROM with region/system nibble `$7`. Game Gear-only `gg.*` modules provide 12-bit CRAM colors, the Start button, the centered 160×144 viewport, and PSG stereo routing. Build [`examples/sega-game-gear/source-hello`](../examples/sega-game-gear/source-hello) with:

```sh
cargo run -- build examples/sega-game-gear/source-hello/src/main.ezra
```

The target supports Sega slot-2 mapper control for banked data through `sms.bank`. It does not yet support banked executable code, cartridge SRAM, interrupts, or emulator-backed runtime tests.

## Nintendo Game Boy

Targets:

```text
gameboy-dmg-lr35902
gameboy-color-lr35902
```

These targets support both EZRA source compilation and handwritten assembly through a dedicated LR35902 assembler. They do not inherit Z80-only instructions. By default they produce 32 KiB ROM-only cartridges; `[gameboy]` project configuration also supports MBC1 and MBC5 mapper cartridges with explicit 16 KiB switchable-bank payload files, external RAM, battery, and MBC5 rumble metadata. All cartridges receive the Nintendo logo, entry stub, title, DMG/CGB compatibility flag, and valid header/global checksums. DMG output uses `.gb`; Game Boy Color output uses `.gbc`. Code begins at `0x0150`; the ROM header occupies `0x0100..0x014F`. See `docs/gameboy-assembly.md` for bank-file placement and mapper switching rules.

The assembler covers all 244 executable base instructions and all 256
CB-prefixed instructions. It uses Z80-style parentheses for memory operands,
including Game Boy forms such as `ld a, (hl+)`, `ldh (rSC), a`, `swap a`, and
`stop`. Unsupported Z80 instructions such as `out`, `exx`, IX/IY, and ED
instructions are rejected.

Vendor `toolchains/gameboy-lr35902/sdk/asm/gb` into a project and include
`hardware.inc`, or `color.inc` for CGB helpers. The macro SDK includes hardware
registers and common patterns for interrupts, LCD/VRAM, DMA, input, timers,
serial, sound, banking, CGB palettes, memory banks, HDMA, and speed switching.
Hardware timing constraints remain explicit.

See `docs/gameboy-assembly.md` for a complete assembly example, instruction
syntax, ROM behavior, SDK scope, and hardware caveats.

```toml
[build]
input = "src/main.asm"
input_kind = "assembly"
target = "gameboy-dmg-lr35902"
```

## CP/M 2.2

Target patterns:

```text
cpm-2.2-z80
cpm-2.2-i8080
cpm-2.2-i8085
```

Default output is `.com`. The default load and entry address is `0x0100`, the CP/M transient program area start.

Built-in SDK modules:

```text
cpm.bdos
cpm.console
cpm.dma
cpm.fcb
```

Checked-in runnable examples live under `examples/cpm-z80`. Build an assembly example with:

```sh
cargo run -- build --target cpm-2.2-z80 --input-kind assembly examples/cpm-z80/console-output.asm
```

Build the source example with:

```sh
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/console-output.ezra
```

Coding guidance:

Use BDOS wrappers rather than direct magic calls in shared code. Keep `.COM` programs within the transient program area and remember that address `0x0005` is the BDOS call vector. See `docs/cpm-sdk-tracker.md` for the CP/M SDK roadmap and current gaps.

## Bare Targets

Target patterns:

```text
bare-ez80
bare-z80
bare-r800
bare-z80n
bare-z180
bare-i8080
bare-i8085
generic-dcpu-bare
bare-tms9900
```

Bare targets use raw `.bin` output and do not enable default SDK symbols. `generic-dcpu-bare` is available with the optional `dcpu` feature for complete standalone DCPU-16 1.7 handwritten assembly; see [`dcpu-assembly.md`](dcpu-assembly.md). `bare-tms9900` is available with the optional `tms9900` feature and supports the initial scalar source backend plus handwritten assembly; see [`tms9900-assembly.md`](tms9900-assembly.md). Layouts are generic:

```text
bare-ez80   24-bit address space, load 0x000000, stack 0xFFFFFF
bare-*      16-bit address space, load 0x0000, stack 0xFFFF
```

Coding guidance:

Provide a custom `.ezralayout` for real hardware. Define your own ports, MMIO addresses, interrupt entry points, and startup conventions. Do not assume a runtime, operating system, or default SDK.

## Writing Portable EZRA

Use conditional declarations for target-specific hardware:

```ezra
@cfg(cpu("ez80"))
pub alias PtrInt = u24

@cfg(any(cpu("z80"), cpu("r800"), cpu("z80n"), cpu("z180"), cpu("i8080"), cpu("i8085")))
pub alias PtrInt = u16
```

Use pointer-width predicates when the exact CPU is less important than the address model:

```ezra
@cfg(pointer_width(24))
pub const FAR_MEMORY: bool = true

@cfg(pointer_width(16))
pub const FAR_MEMORY: bool = false
```

Keep public APIs target-neutral and hide platform calls in private helpers:

```ezra
pub fn print_ok() {
    platform_print_ok()
}

@cfg(target("agonlight-mos-ez80"))
fn platform_print_ok() {
    // call agon console wrapper
}
```

Prefer SDK modules for OS calls, screen access, keyboard input, and exits. Use `port`, `mmio`, and inline `asm` only for functionality that does not belong in a reusable SDK wrapper yet.

## Output Selection By Platform

Target defaults can be overridden in `Ezra.toml`:

```toml
[build]
target = "ti84plusce-ez80"
output = "8xp"
```

Recognized output formats are `bin`, `elf`, `elf32`, `com`, `gaem`, `hex`, `tap`, `gb`, `prg`, `crt`, `8xp`, `8ek`, and `8xk`. Game Boy `.gb` output and Commodore 64 `.prg`/`.crt` output are target-checked. TI `.8xp` protected programs are implemented; `.8ek` and `.8xk` are reserved but currently rejected because flash application packaging is not implemented.

## Adding A New Platform

For a new platform on an existing CPU family:

1. Choose a target triple with a supported CPU component.
2. Add or override a `.ezralayout` with valid address ranges for the CPU address width.
3. Add SDK source files and list their root in `[sdk].paths`, or add a built-in SDK under `toolchains/<target>/sdk` and wire it into `src/compile.rs`.
4. Use `@cfg(target("..."))`, `@cfg(cpu("..."))`, and pointer/address-width predicates for platform-specific declarations.
5. Build first with small programs, then add assembly or emulator tests for hardware behavior.

For a new CPU family, a target profile, memory model, assembler/codegen backend, and executable packaging rules are required before it can be considered supported.
