# Tutorial: custom layout

A custom `.ezralayout` file controls addresses, regions, sections, and linker symbols. This tutorial uses the checked-in TI-99/4A PNG example, which keeps its cartridge header, code, and image data in separate ranges.

## Inspect the layout

From the repository root:

```sh
cargo run -- layout examples/ti99-4a/png-assets/png-assets.ezralayout
```

The layout declares `load`, `entry`, `stack`, regions, sections, and symbols. The project connects it in `examples/ti99-4a/png-assets/Ezra.toml`:

```toml
[build]
target = "ti99-4a-tms9900"

[layout]
file = "png-assets.ezralayout"
```

## Build the example

```sh
cargo run --features tms9900 -- build examples/ti99-4a/png-assets/src/main.ezra
```

Because the source path is inside the project, `ezrac` resolves the relative layout file and writes the target artifact under the example's `target/` directory. The project also configures indexed PNG conversion and places the resulting bytes in `.assets`.

## Make a layout of your own

Start with this shape:

```text
layout demo {
    load 0x010000
    entry 0x010040
    stack 0x0FFF00

    region code 0x010000..0x03FFFF read execute
    region rodata 0x040000..0x04FFFF read
    region ram 0x050000..0x0BFFFF read write

    section .header -> code align 1
    section .text -> code align 16
    section .rodata -> rodata align 16
    section .data -> ram align 16
    section .bss -> ram align 1
    section .assets -> rodata align 1
    section .scratch -> ram align 1

    symbol EZRA_ENTRY_ADDR = 0x010040
}
```

Point `[layout].file` at it or pass `--layout` on a command. Keep the target's startup and SDK-required symbols when customizing a real target. The linker reports section overflow, invalid flags, and address-width errors before packaging.
