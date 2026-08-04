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
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/arguments.ezra
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/file-copy.ezra
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/record-seek.ezra
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/directory-scan.ezra
cargo run -- build --target cpm-2.2-z80 examples/cpm-z80/temporary-file.ezra
```

The assembly examples are `console-output.asm` (BDOS 9 `$`-terminated output),
`exit.asm` (BDOS 0 clean exit), `line-input.asm` (BDOS 10 buffered input), and
`file-read.asm` (FCB open, sequential read, and close). The source examples add
bounded command-tail/token access, record-based file copy, random-record seek,
directory search decoding, and temporary-file cleanup/rename. The corresponding
build artifacts are written below `examples/cpm-z80/target/cpm-2.2-z80`.

Run a generated `.com` file in a CP/M 2.2 emulator by placing it on the emulator's
drive image and invoking its base name. `console-output.com` prints:

```text
Hello from EZRA on CP/M
```

`line-input.com` prints `Type: ` and waits for an edited line. `file-read.com`
opens `README.TXT` on the current drive, reads its first 128-byte record into the
DMA buffer, prints the record's first byte on success, and then exits.

CP/M file examples are record-based: every read or write transfers 128 bytes
through the selected DMA buffer. The SDK does not hide partial-record buffering
or provide byte-stream seek. Copy the command tail before selecting a record or
directory DMA buffer because the initial command-tail area and default DMA are
both at `0x0080`.

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
`cpm.bdos` exposes the standard CP/M 2.2 BDOS function numbers, raw-result
helpers, pointer-safe file operations, user-area access, and record operations.
`cpm.console` exposes character and buffered line I/O, console status, newline,
BDOS 9 `$`-terminated string output by raw address, command-tail copying, token
access, and program exit helpers. `cpm.fcb` exposes 36-byte FCB offsets, whole
8.3 parsing/formatting, validation, wildcard and drive helpers, rename setup,
record positions, and decoded directory entries. `cpm.dma` exposes the default
DMA address plus explicit 128-byte record and directory buffer setup. See
`docs/cpm-sdk-tracker.md` for the full SDK roadmap and CP/M host limits.
