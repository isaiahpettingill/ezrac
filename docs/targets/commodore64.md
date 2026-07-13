# Commodore 64

Use the `commodore64-6502` target to compile EZRA programs for a stock C64 with its MOS 6510 CPU (a 6502-compatible CPU with the C64 memory-mapping port).

```sh
cargo run --features mos6502 -- build examples/commodore64/hello/src/main.ezra
```

The output is a `.prg` file with the little-endian `$080D` load address prefix. It loads below BASIC and begins at `$080D`; start it with a monitor `SYS 2061`, a launcher, or an emulator that supports PRG loading. It intentionally does not inject a BASIC stub, so packaging remains suitable for cartridge/monitor loaders too.

## SDK modules

Import these bundled modules:

| Module | Coverage |
| --- | --- |
| `c64.vic` | VIC-II screen/color RAM, raster, display controls, IRQ registers, and sprites |
| `c64.sid` | SID voices, frequency, pulse width, ADSR, waveforms, and master volume |
| `c64.cia` | CIA keyboard matrix, joystick ports, timers, and interrupt control |
| `c64.memory` | 6510 `$0001` banking for ROM, I/O, character ROM, and all-RAM modes |

The standard C64 I/O configuration is `memory.map_roms_and_io()`. Call it before VIC-II, SID, or CIA access if code previously changed banking. SDK helpers use volatile-style MMIO accesses through EZRA pointers; do not use `map_all_ram()` while accessing `$D000-$DFFF` hardware registers.

`vic.clear(character, color)` fills the default `$0400` screen and `$D800` color RAM. Screen codes are PETSCII screen codes, not ASCII.

## Test integration

The repository has a unit/build test for all bundled modules and an ignored Play96 real-core integration test. Set `PLAY96_C64_CORE` to a compatible C64 libretro core (for example, VICE), then run:

```sh
cargo test --features mos6502 --test libretro_examples c64_example_runs_on_real_core -- --ignored
```
