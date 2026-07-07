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
