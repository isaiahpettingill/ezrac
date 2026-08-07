# Sega Game Gear target

`sega-game-gear-z80` builds 32, 48, 64, 128, or 256 KiB export-Game-Gear ROMs with the `.gg` extension.

The Game Gear CPU, memory map, VDP command interface, tile format, and controller direction/action bits are compatible with the Master System target. The compiler therefore reuses the SMS layout, packager core, and these SDK modules:

- `sms.system`
- `sms.vdp`
- `sms.video`
- `sms.memory`
- `sms.input`
- `sms.bank`

Game Gear-only code lives in `gg.*`:

- `gg.palette`: 12-bit `0x0BGR` CRAM colors, stored as two bytes per color.
- `gg.input`: directional/action input through `sms.input`, plus the active-low Start button on port `$00` bit 7.
- `gg.viewport`: constants and name-table indexing for the centered 160×144 visible area.
- `gg.audio`: SN76489 writes and stereo routing through port `$06`.

Use:

```toml
[build]
target = "sega-game-gear-z80"

[sega]
rom_size_kib = 64
bank_files = ["assets/page2.bin", "assets/page3.bin"]
```

The packager emits the configured ROM capacity, places each bank file in a 16 KiB page starting at page 2, writes `TMR SEGA` at `$7FF0`, calculates the checksum across all pages outside the header, and writes the matching export-Game-Gear system and ROM-size nibbles. For example, 32 KiB uses `$7C` and 64 KiB uses `$7E`.

Build the basic example with:

```sh
cargo run -- build examples/sega-game-gear/source-hello/src/main.ezra
```

The [`banked-scenes`](../../examples/sega-game-gear/banked-scenes) example maps tile scenes from pages 2 and 3 and switches them with the two action buttons.

Generated executable code remains fixed below `$7FF0`; banked executable functions are not supported. Cartridge SRAM, interrupt callbacks, and emulator-backed mapper tests are not implemented yet.
