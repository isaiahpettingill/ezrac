# NES indexed PNG sprite

This example converts `assets/player.png` into one native NES 2bpp CHR tile. Source-generated NES ROMs reserve tile 0 and place configured `.assets` tiles in CHR-ROM starting at tile 1.

```sh
cargo run -p ezrac-cli -- build examples/nes-2a03/png-assets/src/main.ezra
```

Open `target/nes-2a03/png-assets.nes` in an NES emulator.
