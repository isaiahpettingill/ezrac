# Super Nintendo Entertainment System

> **Status: Super alpha.** The target, ROM format, compiler ABI, and `snes.*` SDK are early and may change without compatibility support. Test generated ROMs in an emulator before using them on hardware.

The `snes-5a22` target uses the Ricoh 5A22, a 65C816-compatible CPU, with SNES-specific startup, PPU, DMA, controller, audio-port, timing, and LoROM behavior. The new SDK lives under `toolchains/snes-5a22/sdk/snes`; import modules with `import snes.<module>`.

## SDK modules

- `snes.system`: CPU interrupt control, NMI and auto-joypad control, DMA/HDMA shutdown, and FastROM selection.
- `snes.memory`: WRAM addresses, low-RAM access, and caller-bounded WRAM clear/fill helpers.
- `snes.ppu`: forced blank, brightness, BG mode and screen masks, tile-map and character bases, scrolling, VRAM, CGRAM, and color math registers.
- `snes.dma`: channel register setup, CPU DMA and HDMA masks, and helpers for VRAM, CGRAM, and OAM transfers.
- `snes.input`: auto-read joypad registers and named 16-bit SNES button masks.
- `snes.audio`: CPU-to-SPC700 communication ports and small command/data helpers. It does not include an SPC700 program uploader.
- `snes.timing`: HVBJOY status reads and polling waits for vblank and frame boundaries.

The SDK uses 8-bit MMIO writes and explicit 24-bit addresses where the SNES bus needs a bank byte. Applications should use these wrappers instead of repeating register access in inline assembly.

## Startup and frame loop

1. Call `system.initialize()` while the display is still in reset/blank setup.
2. Call `ppu.force_blank()` before PPU or VRAM setup.
3. Configure `ppu.set_background_mode()`, tile-map bases, screen masks, and CGRAM.
4. Use `dma.copy_to_vram()`, `dma.copy_to_cgram()`, or `dma.copy_to_oam()` during blanking or vblank.
5. Call `system.enable_auto_joypad()` before reading `input.read_controller1()` or `input.read_controller2()`.
6. Enable the display with `ppu.enable_display()` and use `timing.wait_frame()` for a polling loop.
7. Only call `system.enable_nmi()` after installing an NMI-safe handler and update path.

## Memory and register notes

- WRAM is `$7E:0000-$7F:FFFF`; `memory.WRAM_BASE` and `memory.WRAM_END` use 24-bit addresses.
- The CPU-side PPU registers are in `$2100-$213F`.
- CPU DMA and HDMA control are `$420B` and `$420C`; channel registers begin at `$4300` with a `$10` byte stride.
- Auto-read controller values are available at `$4218-$421F` as four little-endian 16-bit values.
- CPU/APU communication uses `$2140-$2143`. Loading and synchronizing a complete SPC700 driver remains outside this first SDK pass.
- SNES VRAM addresses are word addresses. `dma.copy_to_vram()` selects interleaved low/high-byte DMA and expects an even byte count for complete words.

## Examples

- `examples/snes-5a22/source-hello` is the Ezra source example. It sets a color backdrop, polls controller 1, and has no external assets.
- `examples/snes-5a22/hello-world` is a raw 65C816 assembly startup. The packager adds its internal header, checksum, and vectors.

## ROM and current limits

The initial packager emits a fixed 32 KiB SlowROM LoROM image. Generated code,
near calls, and function pointers stay in bank `$00`; banked declarations are
rejected. The packager writes the internal header at file offset `$7FC0` and
sets native and emulation vectors to `$8000`.

Development tests execute generated code with the `w65c816` reference emulator.
The public `ezrac test` command does not expose a 65C816 VM backend yet. Validation
also covers strict assembly, source-to-assembly integration, LoROM structure, SDK
compilation, and CLI builds. Run `.sfc` artifacts in a SNES emulator for full
system testing. SPC700 program upload, cartridge SRAM, FastROM packaging, larger
multi-bank ROMs, and native SNES image-asset conversion remain future work.
