# SNES 5A22 SDK

This directory contains the first source-only SDK pass for the SNES Ricoh 5A22. The public modules are under `sdk/snes` and cover system control, memory, PPU, DMA, input, audio ports, and timing.

The APIs follow the small register-wrapper style used by the NES SDK while using 65C816/SNES address widths. See `docs/targets/snes.md` and the two examples under `examples/snes-5a22`.
