CP/M Z80 examples
=================

These examples are CP/M `.COM` programs for the `cpm-2.2-z80` target. Each topic
has a hand-written assembly program and an EZRA source program where appropriate.

Build an example:

```sh
cargo run -- build --target cpm-2.2-z80 --input-kind assembly examples/cpm-z80/console-output.asm
```

Build the EZRA source examples:

```sh
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/console-output.ezra
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/line-input.ezra
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/file-read.ezra
```

The assembly examples are `console-output.asm` (BDOS 9 `$`-terminated output),
`exit.asm` (BDOS 0 clean exit), `line-input.asm` (BDOS 10 buffered input), and
`file-read.asm` (FCB open, sequential read, and close). The corresponding build
artifacts are written below `examples/cpm-z80/target/cpm-2.2-z80`.

Run a generated `.com` file in a CP/M 2.2 emulator by placing it on the emulator's
drive image and invoking its base name. `console-output.com` prints:

```text
Hello from EZRA on CP/M
```

`line-input.com` prints `Type: ` and waits for an edited line. `file-read.com`
opens `README.TXT` on the current drive, reads its first 128-byte record into the
DMA buffer, prints the record's first byte on success, and then exits.

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

The SDK currently includes `cpm.bdos`, `cpm.console`, `cpm.dma`, and `cpm.fcb`.
`cpm.bdos` exposes the standard CP/M 2.2 BDOS function numbers, generic BDOS
call helpers, and named wrappers for console, disk, FCB, DMA, user-code, and
random-record calls. `cpm.console` exposes character and buffered line I/O,
console status, newline, BDOS 9 `$`-terminated string output by raw address,
and program exit helpers. `cpm.fcb` exposes File Control Block offsets and setup
helpers. `cpm.dma` exposes the default DMA address and DMA setup helpers. See
`docs/cpm-sdk-tracker.md` for the full SDK roadmap.
