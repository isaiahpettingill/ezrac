# Nintendo Entertainment System

The `nes-2a03` target compiles EZRA source for the Ricoh 2A03 and packages mapper-0 NROM-128 images. Import SDK modules with `import nes.<module>`.

## SDK modules

- `nes.ppu`: rendering control, status, vblank waits, VRAM addresses/data, and scrolling.
- `nes.palette`: background and sprite palette writes plus common color constants.
- `nes.sprites`: direct OAM entry writes, hiding sprites, attributes, and OAM DMA.
- `nes.input`: controller 1 polling and named button masks.
- `nes.audio`: APU disable, channel enable masks, and pulse-channel setup.
- `nes.timing`: one- and two-vblank waits for startup and frame loops.
- `nes.memory`: internal RAM constants and clearing.

Hardware register sequences stay inside the SDK. Applications should use these functions instead of repeating inline assembly. The square example at `examples/nes-2a03/source-hello` contains no application inline assembly.

## Graphics workflow

1. Call `ppu.disable_rendering()` and `audio.disable()` during reset setup.
2. Call `timing.wait_two_vblanks()` before the first PPU upload.
3. Set palettes with `palette.set_background()` and `palette.set_sprite_color()`.
4. Write sprites with `sprites.set()`, or prepare page `$02` and call `sprites.dma()`.
5. Enable background or sprite bits with `ppu.set_mask()`.
6. Use `timing.wait_frame()` for simple frame loops. Larger games should enable NMI after installing an NMI-safe update path.

Source-generated ROMs reserve one solid 8×8 tile in CHR tile 0. Configured `.assets` embeds are packed into CHR-ROM as whole 16-byte tiles starting at tile 1. The indexed PNG pipeline can generate those bytes directly; see `examples/nes-2a03/png-assets`. Raw assembly ROMs continue to own their full CHR image.

## Source references

Issue [#89](https://github.com/isaiahpettingill/ezrac/issues/89) defines the required NES source corpus and license rules. This initial SDK uses clean-room register behavior from NES hardware conventions and the repository's existing ROM fixture. No third-party source was copied into these modules. Before adapting code from the issue corpus, pin the source revision, recheck its license, and add attribution and notices required by that revision.

The existing `examples/nes-2a03/hello-world` assembly is based on Thomas Wesley Scott's tutorial source and remains a separate handwritten-ROM fixture. GPL and unlicensed projects listed in #89 are reference-only unless redistribution is reviewed first.

## Current limits

- Mapper 0, one 16 KiB PRG bank, and one 8 KiB CHR bank only. Source assets have 8,176 bytes available after reserved tile 0.
- Generated reset, NMI, and IRQ vectors currently point to `$C000`; source programs should leave NMI disabled unless their startup path handles it.
- No emulator-backed PPU or controller runner is included yet. Tests validate generated assembly and ROM structure.
