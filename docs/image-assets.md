# Indexed PNG image assets

The CLI can convert indexed-color PNG files in a project into the byte layout used by the selected target. The source still uses a normal byte embed, so SDK upload and drawing functions do not need a separate image type.

## Project configuration

Configure each PNG in `Ezra.toml`:

```toml
[assets]
section = ".assets"
align = 16

[[assets.images]]
path = "assets/player.png"
kind = "sprite"

[[assets.images]]
path = "assets/world-tiles.png"
kind = "tiles"
```

Embed the same paths from Ezra source:

```ezra
embed player: bytes = file("assets/player.png")
embed world_tiles: bytes = file("assets/world-tiles.png")
```

`path` is relative to the project root and cannot escape it. `kind` is `tiles`, `sprite`, or `bitmap`. An embed may use a path relative to its declaring module or the project root. This also works for embeds declared in imported modules.

Only files listed in `assets.images` are converted. Other `file(...)` embeds keep their existing raw-byte behavior.

## PNG rules

- The PNG must use indexed color. Grayscale, RGB, and RGBA PNGs are rejected.
- 1-, 2-, 4-, and 8-bit palette indices are accepted.
- Pixel indices are preserved. There is no color reduction, remapping, dithering, scaling, or tile deduplication.
- Tile sheets are read left to right, then top to bottom.
- Unused palette entries do not count against a target's color limit. Every index used by the image must fit the native format.
- Palette colors are only written by direct-color formats. Planar and 1bpp formats emit pixel data only; upload a target palette separately when the hardware needs one.
- Invalid dimensions fail the build instead of cropping or padding the image, except for the required final padding byte in a C64 sprite.

## Native formats

| Target | Kind | Output |
| --- | --- | --- |
| `gameboy-dmg-lr35902`, `gameboy-color-lr35902` | `tiles`, `sprite` | 8×8 Game Boy 2bpp tiles, low/high plane bytes per row |
| `nes-2a03` | `tiles`, `sprite` | 8×8 NES CHR tiles, eight low-plane bytes followed by eight high-plane bytes |
| `sega-master-system-z80`, `sega-game-gear-z80` | `tiles`, `sprite` | 8×8 mode-4 tiles, four plane bytes per row |
| `commodore64-6502` | `tiles` | 8×8 1bpp row bytes |
| `commodore64-6502` | `sprite` | one 24×21 hires sprite plus the required 64th padding byte |
| `ti99-4a-tms9900`, `zxspectrum-z80*` | `tiles` | 8×8 1bpp row bytes; color/attribute data remains separate |
| `arduboy-atmega32u4` (`arduboy-avr` alias) | `sprite`, `bitmap` | 1bpp vertical pages, one byte per 8-pixel column |
| `ti83-z80`, `ti83plus-z80`, `ti84-z80`, `ti84plus-z80` | `bitmap` | horizontal row-major 1bpp bytes |
| `ti84plusce-ez80`, `ti83premiumce-ez80` | `bitmap` | row-major little-endian RGB565 pixels |
| `agonlight-*` VDP/MOS targets | `sprite`, `bitmap` | row-major RGBA8888 pixels |

Tile formats require width and height to be multiples of 8. Arduboy images require a height that is a multiple of 8. TI Z80 bitmaps require a width that is a multiple of 8. A C64 hires sprite must be exactly 24×21.

Targets or image kinds without a defined native layout are rejected. Bare and text-only targets do not guess a format.

## Examples

Each supported graphics family has a buildable project under `examples`:

- `examples/gameboy/png-assets`
- `examples/nes-2a03/png-assets`
- `examples/sega-master-system/png-assets`
- `examples/sega-game-gear/png-assets`
- `examples/commodore64/png-assets`
- `examples/zxspectrum-z80/png-assets`
- `examples/ti99-4a/png-assets`
- `examples/arduboy/png-assets`
- `examples/ti-z80/png-assets`
- `examples/ti84plusce/png-assets`
- `examples/agon-mos/png-assets`

The examples upload or draw the converted data through their platform SDK or graphics memory. NES source builds reserve CHR tile 0 and pack configured `.assets` into whole 16-byte tiles starting at tile 1.

## Library use

The full in-memory pipeline works with `no_std + alloc`. Use `ezra::image::decode_indexed_png` to decode bytes, `indexed_png_to_native_bytes` for one-step target conversion, or pass an `IndexedImage` to `encode_for_target`. You can also select a `NativeImageFormat` and call `encode_image` directly.

No-std builds never access host paths. A no-std workspace supplies PNG bytes through `WorkspaceFile` or another caller-owned buffer, converts them with `ezra::image`, and embeds the returned native bytes. The CLI adds project-file discovery and automatic conversion of configured embeds, but uses the same core decoder and encoders.
