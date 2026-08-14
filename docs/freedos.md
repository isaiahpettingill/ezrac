# FreeDOS compiler

The FreeDOS port is a separate `no_std + alloc` executable. It contains no test runner, LSP server, editor setup, project discovery, or non-DOS command-line options.

The port runs the regular EZRA parser, i8086 code generator, assembler, linker, and DOS `.COM` packager. It enables the no-std i8086 feature set and leaves out host tools.

## Use

```dos
EZRAC source.ezra
EZRAC source.ezra output.com
```

The first form writes `source.com`. Input must be UTF-8. The command creates a one-file virtual workspace, so project manifests, filesystem imports, and `embed file(...)` are not available.

EZRAC runs as a 32-bit DOS/32A protected-mode executable. Programs it produces are 16-bit `.COM` files for DOS.

## Build requirements

- Rust nightly with `rust-src`
- GNU Make
- NASM
- OpenWatcom with `WATCOM` set to its install directory
- FreeDOS for runtime testing

Build from the repository root:

```sh
make -C dos
```

The result is `dos/target/dos/release/ezrac.exe`.

The build:

1. Builds `core` and `alloc` for the custom 32-bit i486 COFF target.
2. Builds EZRAC as a Rust static library.
3. Compiles a small OpenWatcom C startup entrypoint.
4. Adds NASM DOS syscall and MinGW-compatible stack-probe helpers.
5. Links and binds DOS/32A with OpenWatcom.

The C entrypoint only initializes the Watcom/DOS runtime and calls Rust. The command line, validation, output selection, errors, and compiler behavior are in Rust.

## Runtime memory

The Rust heap requests 4 MiB arenas through DPMI interrupt `31h`, function `0501h`. It is a bump allocator: individual allocations are not reclaimed because every compiler path exits directly through DOS. DOS reclaims all arenas when EZRAC exits.

The linked process uses a 4 MiB stack. A FreeDOS VM with at least 128 MiB is recommended while the port is under development.

## FreeDOS test

Copy `dos/target/dos/release/ezrac.exe` and an EZRA source file into a FreeDOS VM, then run:

```dos
EZRAC HELLO.EZRA HELLO.COM
HELLO.COM
```

DOSBox-X 2026.08.02 runs `EZRAC /?`, but its host process crashes in DOS/32A's protected-mode file-open bridge. Use FreeDOS under QEMU for compiler runtime testing until that emulator issue is resolved.
