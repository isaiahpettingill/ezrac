# Embedded data and image assets

`embed` places immutable data in the program image:

```ezra
embed logo: bytes = file("assets/logo.bin")
embed message: bytes = text("HELLO")
embed c_message: bytes = cstr("HELLO")
embed palette: bytes = bytes [0x00, 0x11, 0x22]
embed padding: bytes = repeat(0, 256)
embed matrix: [u8; 4] = [1, 2, 3, 4]
```

`bytes` remains available as the legacy declaration/source spelling. A typed embed must be a fixed `u8` array whose length matches the produced bytes.

## Sections and alignment

Place an embed in a named section and set alignment:

```ezra
embed tiles: [u8; 2] = [0xA1, 0xA2] section .assets align 256
```

Project defaults can apply to embeds without explicit clauses:

```toml
[assets]
section = ".assets"
align = 16

[assets.targets."gameboy-*"]
section = ".rodata"
align = 16
```

An explicit source `section` or `align` wins over project defaults. The layout must define a region for the selected output section.

## Files and workspace builds

`file("...")` is relative to the source file declaring it. The CLI reads host files. Library callers using an in-memory `Workspace` must provide matching `WorkspaceFile` entries; no-std builds never read host paths.

## Indexed PNG conversion

Configure indexed PNG preprocessing in `Ezra.toml`, then embed the same file:

```toml
[[assets.images]]
path = "assets/player.png"
kind = "sprite"
```

```ezra
embed player: bytes = file("assets/player.png")
```

The target converts supported indexed PNGs to native sprite, tile, or bitmap bytes before compilation. Unconfigured image files are embedded unchanged. See [image assets](../image-assets.md) for target-specific formats and limits.
