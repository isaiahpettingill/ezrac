# MS-DOS 8086 examples

These projects use the built-in `dos.*` SDK and build real-mode MS-DOS `.COM`
programs loaded at offset `0100h`.

Run these commands from the repository root:

```sh
cargo run --features i8086 -- build examples/msdos-i8086/hello/hello.ezra
cargo run --features i8086 -- build examples/msdos-i8086/arguments/arguments.ezra
cargo run --features i8086 -- build examples/msdos-i8086/file-io/file-io.ezra
```

Each artifact is written to the corresponding example's
`target/msdos-com-i8086/` directory.

- `hello` writes a DOS `$`-terminated string.
- `arguments` copies and prints the raw PSP command tail before DOS can reuse
  the default DTA.
- `file-io` creates `NEW.TXT`, writes a message, reopens and prints it, then
  leaves the file in the current directory for inspection.

See [`../../docs/msdos-sdk.md`](../../docs/msdos-sdk.md) for the SDK reference.
