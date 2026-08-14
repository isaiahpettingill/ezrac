# FreeDOS compiler

The FreeDOS port is a `no_std + alloc` compiler pipeline for `msdos-com-i8086`. It contains no test runner, LSP server, editor setup, project discovery, or host-only command-line tools.

## Use

Keep these files in the same directory:

- `EZRAC.BAT`
- `EZFE.EXE`
- `EZOPT.EXE`
- `EZCG.EXE`
- `EZAS.EXE`

Compile a source file with an explicit output name:

```dos
EZRAC HELLO.EZRA HELLO.COM
HELLO.COM
```

Input must be UTF-8. The frontend creates a one-file virtual workspace, so project manifests, filesystem imports, and `embed file(...)` are not available. Built-in `dos.*` SDK imports are available.

Programs produced by the pipeline are 16-bit DOS `.COM` files. The four compiler stages run as 32-bit DOS/32A protected-mode executables.

## Pipeline

`EZRAC.BAT` runs four programs:

1. `EZFE.EXE` parses, resolves, and validates EZRA source, then writes `EZRAC.EZI`.
2. `EZOPT.EXE` lowers the program through HIR and TBIR optimization, then writes `EZRAC.EZO`.
3. `EZCG.EXE` emits Intel 8086 assembly to `EZRAC.ASM` without rerunning TBIR optimization.
4. `EZAS.EXE` assembles and packages the requested `.COM` file.

The `.EZI` and `.EZO` files use a compact bincode representation. They are internal, compiler-version-locked files rather than a stable interchange format. Source text, source excerpts, statement spans, debug comments, and optimization explanations are not stored.

Each stage exits before the next starts. DOS therefore reclaims that stage's stack and heap instead of keeping the complete compiler pipeline in memory.

The stages can also be run separately:

```dos
EZFE SOURCE.EZRA PROGRAM.EZI
EZOPT PROGRAM.EZI PROGRAM.EZO
EZCG PROGRAM.EZO PROGRAM.ASM
EZAS PROGRAM.ASM PROGRAM.COM
```

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

The results are in `dos/target/dos/release`.

## FreeDOS test CD

The complete compiler and hello-world example fit on one ISO image:

```sh
make -C dos test-cd
```

This writes `dos/target/freedos-cd/ezrac-freedos.iso`. Mount it in a FreeDOS VM with CD-ROM and MSCDEX support, switch to the CD drive, and run `INSTALL.BAT`. The installer copies the compiler to `C:\EZRAC`, then compiles and runs `HELLO.EZR`. The image is not bootable.

## FreeDOS test floppies

The compiler is too large for one standard floppy. Build a four-disk 2.88 MiB FAT12 test set after building the compiler:

```sh
make -C dos test-floppies
```

The images are written to `dos/target/freedos-floppies` as `ezrac-1.img` through `ezrac-4.img`. Attach `ezrac-1.img` to the FreeDOS VM, switch to drive `A:`, and run:

```dos
INSTALL.BAT
```

The installer copies itself to `C:\EZRAC`, prompts for the other three images, then compiles and runs the included `HELLO.EZR` DOS SDK example. The floppy uses the three-character `.EZR` extension because plain FreeDOS filenames use the 8.3 format. The images are not bootable; start the VM from its existing FreeDOS installation.

The build compiles each Rust stage as a separate static library, compiles one OpenWatcom C startup entrypoint, adds NASM DOS syscall and MinGW-compatible stack-probe helpers, then links and binds DOS/32A with OpenWatcom. The DOS release profile uses Rust's `z` size optimization; current stage executables are about 2.4–2.5 MiB each.

## Runtime memory

The Rust heap requests 4 MiB arenas through DPMI interrupt `31h`, function `0501h`. It is a bump allocator: individual allocations are not reclaimed because each stage exits directly through DOS. DOS reclaims all arenas when a stage exits.

Each linked process uses a 4 MiB stack. A FreeDOS VM with at least 128 MiB is recommended while the port is under development.

## Emulator note

DOSBox-X 2026.08.02 loads the DOS/32A programs, but its host process crashes when DOS/32A forwards the protected-mode file-open interrupt. This occurs after executable startup and is not caused by executable size. Use FreeDOS under QEMU for compiler runtime testing until that emulator issue is resolved.
