# Master System banked scenes

This example builds a 64 KiB `.sms` ROM. `assets/scene-a.bin` is ROM page 2 and `assets/scene-b.bin` is ROM page 3. Each page contains four mode-4 tiles. The program maps the selected page into slot 2, uploads its tiles to VRAM, and redraws the name table.

Build:

```sh
cargo run -- build examples/sega-master-system/banked-scenes/src/main.ezra
```

Controls:

- Button 1: scene A from page 2
- Button 2: scene B from page 3

Run 120 frames in Genesis Plus GX through `play96`, pressing button 2 after frame 30:

```sh
play96-cli --core genesis_plus_gx_libretro.dll \
  --cart examples/sega-master-system/banked-scenes/target/sega-master-system-z80/banked-scenes.sms \
  --frames 120 --shot-every 120 --macro "30-119:p1.a"
```
