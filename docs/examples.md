# Examples index

All commands below run from the repository root. Replace `cargo run --` with `ezrac` after installing the CLI.

## Agon MOS / eZ80

- [`agon-mos/hello`](../examples/agon-mos/hello/) — smallest Agon project and build output.
- [`agon-mos/console`](../examples/agon-mos/console/) — console input/output.
- [`agon-mos/coffee-order`](../examples/agon-mos/coffee-order/) — interactive menu; used by the [coffee-order tutorial](tutorials/agon-mos-coffee-order.md).
- [`agon-mos/sdk-showcase`](../examples/agon-mos/sdk-showcase/) — bundled SDK calls.
- [`agon-mos/mandelbrot`](../examples/agon-mos/mandelbrot/) — arithmetic and a long-running render loop.
- [`agon-mos/space-invaders`](../examples/agon-mos/space-invaders/) — larger input, state, and rendering example.
- [`agon-mos/png-assets`](../examples/agon-mos/png-assets/) — indexed PNG conversion and `embed file(...)`.

Build one:

```sh
cargo run -- build examples/agon-mos/coffee-order/src/main.ezra
```

## Z80-family targets

- [`bare-z80`](../examples/bare-z80/) — bare target and loop/control-flow example.
- [`zxspectrum-z80`](../examples/zxspectrum-z80/) — Spectrum graphics, input, and sound.
- [`cpm-z80`](../examples/cpm-z80/) — CP/M source and assembly programs.
- [`ez180n`](../examples/ez180n/) — console demos and small games.

```sh
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/console-output.ezra
cargo run -- build --target cpm-2.2-z80 --input-kind assembly examples/cpm-z80/console-output.asm
```

## Game consoles

- [`gameboy`](../examples/gameboy/) — DMG/CGB source programs, sprites, audio, and banking.
- [`nes-2a03`](../examples/nes-2a03/) — source and raw assembly NROM-128 examples.
- [`snes-5a22`](../examples/snes-5a22/) — source and 65C816 assembly LoROM examples.
- [`commodore64`](../examples/commodore64/) — C64 source, sprites, raster bands, and KERNAL calls.
- [`sega-master-system`](../examples/sega-master-system/) and [`sega-game-gear`](../examples/sega-game-gear/) — banking and target SDK examples.

## Other CPUs

- [`arduboy`](../examples/arduboy/) — AVR source and graphics/input examples.
- [`bare-avr`](../examples/bare-avr/) — generic AVR source examples.
- [`bare-i8086`](../examples/bare-i8086/) and [`msdos-i8086`](../examples/msdos-i8086/) — 8086 source and DOS `.COM` examples.
- [`bare-6502`](../examples/bare-6502/) — MOS 6502 source examples.
- [`pic18`](../examples/pic18/) — PIC18 source and emulator-tested code.
- [`dcpu-16`](../examples/dcpu-16/) — DCPU-16 source, SDK, and handwritten assembly.
- [`bare-m68000`](../examples/bare-m68000/) — experimental M68k source.
- [`bare-tms9900`](../examples/bare-tms9900/) and [`ti99-4a`](../examples/ti99-4a/) — TMS9900 source and cartridge examples.

## Toolchain examples

The repository also contains [`register-allocation`](../examples/register-allocation/) and [`tiny-lisp`](../examples/tiny-lisp/) for compiler and language experiments. Their README files are the source of truth for their current command lines.

When adding an example, include its target in `Ezra.toml`, a short README, and a root-relative build command. Then add it here.
