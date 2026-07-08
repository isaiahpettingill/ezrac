CP/M Z80 examples
=================

These examples are CP/M `.COM` programs for the `cpm-2.2-z80` target. Most are
small assembly fixtures; `hello-source.ezra` is an EZRA source example using the
built-in `cpm.console` SDK.

Build an example:

```sh
cargo run -- assemble --target cpm-2.2-z80 examples/cpm-z80/hello-char.asm
```

Build the source example:

```sh
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/hello-source.ezra
```

The default output extension for `cpm-2.2-z80` is `.com`, and the default Z80
assembly base is `0x0100`, the CP/M `.COM` load address.

SDK modules are available for EZRA source imports on the CP/M target:

```ezra
import cpm.console

fn main() {
    console.write(65)
    console.newline()
    console.exit()
}
```

The SDK currently includes `cpm.bdos` and `cpm.console`. `cpm.bdos` exposes the
standard CP/M 2.2 BDOS function numbers, generic BDOS call helpers, and named
wrappers for console, disk, FCB, DMA, user-code, and random-record calls.
`cpm.console` exposes character I/O, console status, newline, BDOS 9
`$`-terminated string output by raw address, and program exit helpers. See
`docs/cpm-sdk-tracker.md` for the full SDK roadmap.
