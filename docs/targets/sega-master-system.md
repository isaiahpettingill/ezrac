# Sega Master System target design

**Status: mapper-capable data banking implemented.** `sega-master-system-z80` builds 32, 48, 64, 128, or 256 KiB export-SMS ROMs. It includes a standard header, reset/VBlank/NMI vector stubs, and a small polling-based `sms.*` SDK. `sega-game-gear-z80` reuses the common layout and SDK code, emits `.gg` ROMs, and adds Game Gear palette, Start button, viewport, and stereo helpers. Banked executable code, VBlank interrupts, and SRAM remain proposed.

## Implemented first pass

- The `.sms` packager emits the configured 32, 48, 64, 128, or 256 KiB capacity, pads unused ROM with `$FF`, writes `TMR SEGA` at `$7FF0`, and calculates the standard checksum across all ROM pages outside the header.
- `[sega] bank_files` place ordered 16 KiB payloads in ROM pages 2 and later. The shared `sms.bank` module selects pages in slot 2 and supports copy-with-restore.
- Reset at `$0000` jumps to generated code at `$0069`. `$0038` contains `RETI` and `$0066` contains `RETN`; VBlank interrupts stay disabled.
- The bundled SDK provides `sms.vdp`, `sms.video`, `sms.palette`, `sms.system`, `sms.memory`, and `sms.input`.
- `sms.input` supports two standard SMS pads. `read_player1()` and `read_player2()` return active-high `UP`, `DOWN`, `LEFT`, `RIGHT`, `BUTTON_1`, and `BUTTON_2` masks.
- `sms.system.wait_vblank()` polls VDP status. `halt_until_frame()` is an alias and does not execute the Z80 `HALT` instruction.
- See [`examples/sega-master-system/source-hello`](../../examples/sega-master-system/source-hello), [`examples/sega-master-system/banked-scenes`](../../examples/sega-master-system/banked-scenes), [`examples/sega-game-gear/source-hello`](../../examples/sega-game-gear/source-hello), and [`examples/sega-game-gear/banked-scenes`](../../examples/sega-game-gear/banked-scenes).

## Scope

The completed target design supports stock Sega Master System hardware with:

- Z80 at 3.58 MHz, 16-bit addresses, and 8 KiB system RAM.
- The standard Sega 16 KiB-page mapper.
- VDP mode 4: a 256×192 visible area, 8×8 four-plane tiles, two 16-color palettes, 64 sprites, and an eight-sprites-per-scanline limit.
- SN76489 PSG sound.
- Two standard SMS controllers and the console Pause button.
- ROMs from 32 KiB through 256 KiB. 512 KiB and 1 MiB are a packager extension once the baseline is tested.

Generated executable code remains fixed below the header, while explicit data files can use Sega mapper pages. It excludes cartridge SRAM, FM sound, light guns, paddles, 3-D glasses, Codemasters mappers, and banked executable code. Game Gear output is supported by the separate `sega-game-gear-z80` target. These other features need explicit implementation rather than silently producing an incompatible ROM.

## Target and project configuration

Use this triple:

```text
sega-master-system-z80
```

The target uses the existing Z80 assembler and source backend. The target provides a shared CPU layout, mapper-capable ROM packager, and small SDK modules.

ROM capacity and ordered bank files use the shared `[sega]` table on both SMS and Game Gear targets:

```toml
[build]
target = "sega-master-system-z80"
output = "sms"
```

Example project configuration:

```toml
[project]
name = "my-sms-game"

[build]
target = "sega-master-system-z80"
output = "sms"
executable = "my-sms-game"

[sega]
rom_size_kib = 64
bank_files = ["assets/page2.bin", "assets/page3.bin"]

[optimization]
level = 2
```

The default capacity is 32 KiB. `rom_size_kib` is an explicit capacity, not a request to infer the smallest file. The packager rejects an image that exceeds it, fills unused bytes with `$FF`, and always emits exactly the configured size.

## Hardware map

### CPU address map

| CPU address | Meaning |
| --- | --- |
| `$0000-$3FFF` | ROM slot 0; the runtime keeps this mapped to page 0 |
| `$4000-$7FFF` | ROM slot 1; selected by `$FFFE` |
| `$8000-$BFFF` | ROM slot 2; selected by `$FFFF` |
| `$C000-$DFFF` | 8 KiB work RAM |
| `$E000-$FFFF` | Mirror of `$C000-$DFFF`; do not allocate application state here |
| `$FFFC-$FFFF` | Mapper registers, overlaid on the RAM mirror |

The mapper register addresses are:

| Address | Register | Initial SDK use |
| --- | --- | --- |
| `$FFFC` | RAM control | Write `$00`; SRAM is not supported in the first target |
| `$FFFD` | Slot 0 page | Write `0` during reset, then leave unchanged |
| `$FFFE` | Slot 1 page | Managed by `sms.bank` |
| `$FFFF` | Slot 2 page | Managed by `sms.bank` |

The first 1 KiB of the CPU map has special mapper behavior on real SMS hardware. The runtime never remaps slot 0, so reset and interrupt code remain reachable and applications never need to rely on that exception.

### Controllers

The SMS has two standard controller ports. Inputs are active-low in hardware and are read through `$DC` and `$DD`; `sms.input` inverts them and returns active-high masks. The console Pause button is not a pad input: it invokes the NMI vector at `$0066`.

| Player | `$DC` bits | `$DD` bits |
| --- | --- | --- |
| 1 | 0-5: Up, Down, Left, Right, Button 1, Button 2 | — |
| 2 | 6-7: Up, Down | 0-3: Left, Right, Button 1, Button 2 |

### I/O ports

| Port | Read | Write |
| --- | --- | --- |
| `$3F` | — | I/O control |
| `$7E` | VDP vertical counter | PSG data |
| `$7F` | VDP horizontal counter | PSG data |
| `$BE` | VDP data | VDP data |
| `$BF` | VDP status | VDP control |
| `$DC` | Controller port A/B | — |
| `$DD` | Controller port B/miscellaneous | — |

The SDK owns VDP command sequencing. Application code can use typed port I/O for a missing feature, but normal graphics code should go through `sms.vdp`, `sms.video`, and `sms.sprite`.

## SDK modules

Bundled modules use the `sms.*` namespace.

| Module | Responsibilities |
| --- | --- |
| `sms.system` | reset-facing helpers, region constants, frame counters, and `halt_until_frame()` |
| `sms.vdp` | VDP register writes, status reads, VRAM/CRAM address setup, and bounded byte transfers |
| `sms.video` | mode-4 setup, display enable, scroll position, name-table entries, and tile uploads |
| `sms.palette` | SMS `--BBGGRR` colors, background/sprite palette uploads, and color constants |
| `sms.sprite` | a RAM shadow sprite table, hide/clear, sprite entries, and VBlank commit |
| `sms.input` | active-low controller polling, held/pressed/released state, and Player 1/2 button masks |
| `sms.psg` | SN76489 tone/noise/volume writes and mute |
| `sms.bank` | slot-2 page selection and banked-data copies that restore the prior page |
| `sms.memory` | work-RAM ranges, clear helpers, and runtime-reserved areas |
| `sms.assets` | asset descriptors and VBlank-safe VRAM upload queueing |

The baseline public API should be small. For example:

```ezra
import sms.input
import sms.palette
import sms.sprite
import sms.system
import sms.video

fn main() {
    video.init_mode4()
    palette.set_background(0, 0)
    system.enable_frame_interrupts()
    video.enable_display()

    loop {
        system.halt_until_frame()
        input.poll()
        // Update game state. Queue VRAM and sprite changes here.
    }
}
```

`main()` runs with interrupts disabled until `video.init_mode4()` has installed safe VDP state. The runtime owns reset and vectors; application code supplies `main()`, and may optionally register `on_vblank()` and `on_pause()` callbacks. It must not define code at `$0000`, `$0038`, or `$0066`.

## Runtime ABI and RAM layout

The runtime uses IM 1. The VDP frame interrupt enters at `$0038`; the console Pause button is an NMI at `$0066`.

```text
$0000-$0037  Reset entry and RST stubs
$0038-$0065  IM 1 VBlank handler
$0066-$0068  NMI jump stub
$0069-$03FF  fixed runtime helpers
$0400-$3FFF  application fixed code, constants, and bank trampolines
```

The reset sequence must:

1. Execute `di` and `im 1`.
2. Map Sega pages 0, 1, and 2 to slots 0, 1, and 2, and disable cartridge RAM through `$FFFC`.
3. Set `SP` to `$DFF0` or lower.
4. Clear runtime and application work RAM, excluding the stack and mapper-register mirror addresses.
5. Disable the display and VBlank interrupts; initialize mode-4 VDP registers and clear the sprite shadow table.
6. Call `main()`.
7. If `main()` returns, disable interrupts and loop forever.

The VBlank handler must acknowledge the VDP status read first. It increments the frame counter, commits the sprite shadow table, runs only a bounded queued VRAM/CRAM transfer budget, then invokes an optional application callback. Do not allow direct bank switches, unbounded decompression, input polling, or game simulation in this handler.

A proposed RAM ownership map:

| Range | Owner | Purpose |
| --- | --- | --- |
| `$C000-$C01F` | runtime | frame count, input state, VDP transfer queue state, current pages |
| `$C020-$C11F` | `sms.sprite` | 256-byte sprite attribute shadow and bookkeeping |
| `$C120-$C3FF` | runtime | VBlank upload queue and scratch; exact size may change |
| `$C400-$DBEF` | application | globals, arrays, and dynamic game state |
| `$DBF0-$DFEF` | application | optional transient arena |
| `$DFF0-$DFFF` | runtime | descending Z80 stack |

The compiler layout must reserve the runtime ranges and reject source-visible static storage that overlaps them. `$E000-$FFFF` is never a separate storage region because it mirrors RAM and contains the mapper registers.

## ROM layout

ROM files are split into physical 16 KiB pages. Page number `n` starts at file offset `n * $4000`. Startup mappings are page 0 at `$0000`, page 1 at `$4000`, and page 2 at `$8000`.

```text
File page 0, $00000-$03FFF  fixed runtime, vectors, fixed code, common data
File page 1, $04000-$07FEF  default slot-1 code/data
File offset  $07FF0-$07FFF  mandatory SMS header
File page 2, $08000-$0BFFF  default slot-2 code/data
File page 3+,                banked code, maps, graphics, music, and dialogue
```

The header lives at physical file offset `$7FF0`, which maps to CPU address `$7FF0` under the default page mapping. Reserve it even for 32 KiB ROMs. It is why the linker must treat page 1's final 16 bytes as unavailable. This matches the inspected 32 KiB, 48 KiB, 64 KiB, and 256 KiB homebrew releases.

The linker has four logical placement classes:

| Class | Allowed pages | Purpose |
| --- | --- | --- |
| `fixed` | page 0 only | vectors, runtime, VBlank/NMI callbacks, mapper trampolines, data needed during a bank switch |
| `slot1` | any page, mapped at `$4000` | regular banked code and data; page 1 is the initial slot-1 page |
| `slot2` | any page, mapped at `$8000` | streamed assets and bulk level/music data; page 2 is the initial slot-2 page |
| `rom` | any non-reserved page | packager-selected assets; accessed only through a banked descriptor |

The first compiler version should place all generated application code in `fixed` and use `slot2` for explicit assets. Banked executable code and function pointers need a bank-aware calling convention, so they should follow only after asset streaming is working.

### Bank-aware data

A banked asset descriptor must contain both a page and its CPU-visible address. A raw `u16` pointer is not sufficient.

```text
BankedRef:
  page: u8
  slot: u8       ; 1 or 2
  address: u16   ; $4000-$7FEF or $8000-$BFFF
  length: u16
```

`sms.bank.copy_from_slot2(ref, destination, length)` saves the current slot-2 page, maps `ref.page`, copies the requested bytes, then restores the saved page. The save/restore sequence runs with interrupts disabled unless the VBlank handler is guaranteed not to touch the same slot. The initial SDK should use slot 2 for queued/decompressed asset reads and keep slot 1 stable during a frame.

Banked callbacks need a separate `BankedFn` descriptor and a fixed-page trampoline that maps the page, calls the address, and restores the mapping. Do not expose `BankedFn` to normal EZRA function-pointer syntax until code generation preserves this contract.

### Header and checksum

The packager writes this 16-byte header at `$7FF0` after linking:

| File offset | Size | Content |
| --- | ---: | --- |
| `$7FF0` | 8 | ASCII `TMR SEGA` |
| `$7FF8` | 2 | reserved, `$FF $FF` |
| `$7FFA` | 2 | little-endian checksum |
| `$7FFC` | 3 | BCD product code and version nibble |
| `$7FFF` | 1 | region/system nibble and ROM-size nibble |

The checksum is the 16-bit sum of file bytes `$0000-$7FEF`, before writing the header. This is the convention verified in the inspected headered homebrew ROMs. The size nibble must describe the configured capacity, not the payload length:

| Capacity | Size nibble |
| ---: | --- |
| 32 KiB | `$C` |
| 48 KiB | `$D` |
| 64 KiB | `$E` |
| 128 KiB | `$F` |
| 256 KiB | `$0` |
| 512 KiB | `$1` |
| 1 MiB | `$2` |

Use region nibble `$4` for export SMS and `$3` for Japanese SMS. The packager must test the exact header bytes, checksum, capacity, header reservation, and page alignment.

## VDP layout and asset format

Use this default mode-4 VRAM layout:

| VRAM range | Use |
| --- | --- |
| `$0000-$1FFF` | background pattern tiles 0-255 |
| `$2000-$37FF` | sprite pattern tiles 256-447 |
| `$3800-$3EFF` | 32×28 name table |
| `$3F00-$3FFF` | sprite attribute table |

A mode-4 tile is 32 bytes: 8 rows × 4 bitplanes. A name-table entry is a little-endian `u16`: tile number in bits 0-8, horizontal flip in bit 9, vertical flip in bit 10, palette in bit 11, and priority in bit 12. Background palettes use color indices 0-15 and sprite palettes use 16-31; SMS color bytes are `--BBGGRR`.

The asset pipeline should convert source art outside the compiler and store generated binary files under the project `assets/` directory:

```text
assets/
  tiles/*.sms4bpp
  maps/*.smsmap
  palettes/*.smscram
  music/*.psg
  sfx/*.psg
```

Generated assets are not source files. Project conversion scripts own PNG/TMX/Furnace input and should create reproducible files in `target/` or another ignored build directory. `embed` metadata associates a generated file with a placement class and emits a `BankedRef` when it is not fixed.

Uploads larger than the VBlank budget must be split across frames. The SDK must expose the queue cost in bytes so games can avoid VDP overrun. Sprite rendering uses the RAM shadow table and one VBlank commit, not piecemeal writes to `$3F00` while the display is active.

## Evidence and source rules

This design is clean-room. The following free-to-download homebrew archives were inspected locally but are not included in this repository:

- `Stalactites-SMS-0.60.zip` — 32 KiB; `devkitSMS` startup marker and valid header.
- `Sub-Assault V1.4.1.zip` — 32-64 KiB development history; release uses `devkitSMS` startup, Sega banking, and SRAM for scores.
- `Astro Climber (v1.2.1).zip` — 64 KiB; `devkitSMS` startup marker but no standard header.
- `FlightOfPigarus-SMS-1.10.zip` — 256 KiB; custom Sega mapper initialization and a valid header.
- `SilverValley-SMS-1.00.zip` — 256 KiB; custom mapper initialization and banked dialogue data.

The ROMs show that a fixed startup page, `$FFFC-$FFFF` mapper setup, `$0038` IM 1 service, `$0066` NMI service, a header at `$7FF0`, and 16 KiB data pages are practical release patterns. They do not grant permission to copy code, graphics, music, text, or binary assets. Tests may record ROM sizes, public header facts, and clean-room expected output, but must not add ROM bytes, ROM hashes, extracted assets, or derived disassemblies to the repository.

Reference material used for hardware behavior:

- [SMS Tributes: Z80 Assembly programming for SMS and Game Gear](https://www.smstributes.co.uk/view_article.asp?articleid=40)
- [ChibiAkumas: Master System and Game Gear](https://www.chibiakumas.com/z80/MasterSystemGameGear.php)
- [Maxim: SMS Programming, Lesson 1](https://www.smspower.org/maxim/HowToProgram/Lesson1AllOnOnePage)

## Implementation order

1. Add target recognition, a 64 KiB CPU layout, `.sms` output selection, and the 32 KiB packager/header unit tests.
2. Add `toolchains/sega-master-system-z80/` with reset, IM 1/NMI vectors, RAM clear, VDP setup, and the `sms.system`, `sms.vdp`, `sms.video`, and `sms.memory` modules.
3. Add a 32 KiB hello-world example that uploads a palette, tiles, and name-table data and runs on an SMS emulator/core.
4. Add `sms.input`, `sms.sprite`, and `sms.psg`, then test an interrupt-driven frame loop.
5. ~~Add 48/64/128/256 KiB packing and `sms.bank` slot-2 asset streaming.~~ Implemented for ordered bank files and copy-with-restore; emulator mapper testing remains.
6. Add an asset converter contract and a scrolling/tilemap example.
7. Add optional cartridge SRAM, Game Gear, FM sound, and alternate mappers only behind explicit project configuration.

The target starts at Tier 4. It moves to Tier 3 only after build, header, mapping, and emulator/core smoke tests pass; it moves to Tier 2 after runtime and SDK behavior are covered by automated tests.