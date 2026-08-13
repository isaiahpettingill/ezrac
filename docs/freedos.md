# FreeDOS compiler

The FreeDOS port is a separate `no_std + alloc` executable. It contains only the EZRA compiler path needed for the `msdos-com-i8086` target. It does not include the test runner, LSP server, project discovery, editor setup, disk-image commands, or non-i8086 backends.

## Use

```dos
EZRAC source.ezra
EZRAC source.ezra output.com
```

The first form writes `source.com`. The DOS frontend accepts one UTF-8 EZRA source file. Imports and `embed file(...)` are not yet supported because this first frontend builds a one-file virtual workspace.

EZRAC itself runs as a 32-bit DOS/32A protected-mode executable. Programs produced by it remain 16-bit `.COM` files for the `msdos-com-i8086` target.

## Build requirements

- Rust nightly with `rust-src`
- GNU Make
- NASM
- OpenWatcom with `WATCOM` set to its install directory
- DOSBox or a FreeDOS environment for runtime testing

Build from the repository root:

```sh
make -C dos
```

The result is `dos/target/dos/release/ezrac.exe`. The build compiles `core` and `alloc` for the custom i486 DOS target, builds EZRAC as a Rust static library, adds a 64 KiB 32-bit stack segment, and links it with OpenWatcom using `system dos32x`.

The runtime calls DOS interrupt `21h` for arguments, files, console output, and process exit. The Rust heap gets 1 MiB arenas through DPMI interrupt `31h`, function `0501h`, and reuses freed blocks within those arenas. Compiler data therefore uses the DOS extender's 32-bit memory instead of conventional DOS memory. DOS reclaims the arenas when EZRAC exits.

## FreeDOS test

Copy these files into the same FreeDOS directory:

- `dos/target/dos/release/ezrac.exe`
- a small `.ezra` source file

Then run:

```dos
EZRAC HELLO.EZRA HELLO.COM
HELLO.COM
```

DOS/32A must be available to the executable. OpenWatcom's `system dos32x` output normally includes the bound extender setup expected by the `dos-rs` link method.
