CP/M Z80 examples
=================

These examples are CP/M `.COM` programs for the `cpm-2.2-z80` target. They are
assembly-only until EZRA source codegen has a Z80 backend.

Build an example:

```sh
cargo run -- assemble --target cpm-2.2-z80 examples/cpm-z80/hello-char.asm
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
wrappers for console, disk, FCB, DMA, user-code, and random-record calls. EZRA
source builds for `cpm-2.2-z80` still require the pending Z80 source backend;
the checked-in runnable examples are assembly-only for now.
