# MS-DOS 8086 examples

These examples exercise the built-in `dos.*` SDK and compile as real-mode
MS-DOS `.COM` programs loaded at offset `0100h`.

Run from the repository root:

```sh
cargo run --features i8086 -- build --target msdos-com-i8086 examples/msdos-i8086/hello.ezra
cargo run --features i8086 -- build --target msdos-com-i8086 examples/msdos-i8086/arguments.ezra
cargo run --features i8086 -- build --target msdos-com-i8086 examples/msdos-i8086/file-io.ezra
```

Artifacts are written under `examples/msdos-i8086/target/msdos-com-i8086/`.

- `hello.ezra` writes a DOS `$`-terminated string.
- `arguments.ezra` immediately copies and prints the raw PSP command tail.
- `file-io.ezra` creates, writes, closes, reopens, reads, prints, and deletes a file.

The command tail normally begins with the shell-supplied separator. It is not
NUL-terminated in the PSP and shares storage with DOS's default DTA, so copy it
before directory searches. See [`../../docs/msdos-sdk.md`](../../docs/msdos-sdk.md).
