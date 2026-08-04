# NES source example

This example compiles EZRA source for the NES Ricoh 2A03 CPU and packages it as an NROM-128 `.nes` image.

```sh
ezrac build examples/nes-2a03/source-hello/src/main.ezra
```

The generated ROM uses one 16 KiB PRG bank, one 8 KiB CHR bank with a built-in solid tile, mapper 0, and reset/NMI/IRQ vectors that point to the EZRA startup code at `$C000`. The program initializes the PPU and draws the solid tile as a white 8×8 sprite near the center of a dark-blue screen.
