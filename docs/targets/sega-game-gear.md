# Sega Game Gear target

`sega-game-gear-z80` builds fixed 32 KiB export-Game-Gear ROMs with the `.gg` extension.

The Game Gear CPU, memory map, VDP command interface, tile format, and controller direction/action bits are compatible with the Master System target. The compiler therefore reuses the SMS layout, packager core, and these SDK modules:

- `sms.system`
- `sms.vdp`
- `sms.video`
- `sms.memory`
- `sms.input`

Game Gear-only code lives in `gg.*`:

- `gg.palette`: 12-bit `0x0BGR` CRAM colors, stored as two bytes per color.
- `gg.input`: directional/action input through `sms.input`, plus the active-low Start button on port `$00` bit 7.
- `gg.viewport`: constants and name-table indexing for the centered 160×144 visible area.
- `gg.audio`: SN76489 writes and stereo routing through port `$06`.

Use:

```toml
[build]
target = "sega-game-gear-z80"
```

The packager emits a 32 KiB ROM, writes `TMR SEGA` at `$7FF0`, calculates the checksum, and uses header byte `$7C` for an export Game Gear with a 32 KiB capacity.

Build the example with:

```sh
cargo run -- build examples/sega-game-gear/source-hello/src/main.ezra
```

Mapper control, larger ROMs, cartridge SRAM, interrupt callbacks, and emulator-backed runtime tests are not implemented yet.
