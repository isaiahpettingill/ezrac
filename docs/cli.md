# CLI reference

Use `ezrac` after installation. From a checkout, use `cargo run --` instead.

Run `ezrac --help` for the command list. The commands below show the current interface.

## Project and inspection commands

```sh
ezrac init [--name <name>] [--target <triple>] [--force] [dir]
ezrac targets
ezrac layout [file.ezralayout]
ezrac header
ezrac install-syntax (--all | [--editor] <editor>...) [--dry-run]
```

`init` creates `Ezra.toml`, `README.md`, `src/main.ezra`, `sdk/.gitkeep`, `assets/.gitkeep`, and a `.gitignore` without overwriting files unless `--force` is used. `targets` prints documented triples and support status. `layout` prints the default or selected layout. `header` prints the default 64-byte EZRA cartridge header.

## Source commands

```sh
ezrac check [options] <file.ezra>
ezrac emit-asm [options] <file.ezra>
ezrac emit-ir [--stage hir|tbir] [options] <file.ezra>
ezrac build [options] [file.ezra|file.asm]
ezrac test [options] [file.ezra]
```

Common options are:

- `--target <triple>` — select a target profile.
- `--layout <file.ezralayout>` — replace the target's default layout.
- `--debug-comments` — add extra comments to generated assembly.
- `--no-default-sdk-symbols` — disable automatic target SDK/runtime symbols.
- `-O0` through `-O3` — select source optimization level; the default is `-O2`.
- `--enable-optimization <pass>` and `--disable-optimization <pass>` — override individual optimization passes.

`build` also accepts `--cpu <mode>`, `--input-kind ezra|assembly`, and repeatable `--size-budget NAME=BYTES`.

### `check`

`check` parses imports, type-checks the program, validates target lowering, and validates generated assembly. It does not write the normal build artifacts.

```sh
cargo run -- check --target custom-unknown-ez80 examples/bare-ez80/src/main.ezra
```

### `emit-asm` and `emit-ir`

Both commands print to standard output only after validation succeeds:

```sh
cargo run -- emit-asm examples/agon-mos/hello/src/main.ezra
cargo run -- emit-ir --stage hir examples/agon-mos/hello/src/main.ezra
cargo run -- emit-ir --stage tbir examples/agon-mos/hello/src/main.ezra
```

### `build`

`build` writes four files under the project's target directory:

```text
<name>.asm
<name>.map
<name>.size
<name>.<target output extension>
```

Use `[build].executable` to choose the basename. `build` does not accept `-o`; use `assemble --output` for a direct assembly output path.

### `test`

```sh
ezrac test [--target <triple>] [--layout <file.ezralayout>] [file.ezra]
```

With a file, it compiles and runs that source on the target VM. Without a file, it loads `Ezra.toml`, discovers `tests/**/*.ezra` in path order, and reports a summary. Target selection is `--target`, then `[test].target`, then `[build].target`, then the compiler default.

The built-in runner supports eZ80 ADL, Z80, Z80N, Z180, R800, i8080, and i8085 through the `ez80` crate. Other targets can still be tested through Rust or third-party emulator integrations; `test` is not a promise of runtime support for every target.

## Assembly commands

```sh
ezrac assemble [--target <triple>] [--cpu <mode>] [--layout <file.ezralayout>] [--map <file.map>] [--base <addr>] [--output <file.bin>] <file.asm>
```

`assemble` produces a raw binary by default. `--base` sets the address used for labels and relocation. `--map` writes a symbol/section map, and `--output` chooses the binary path. Use `build --input-kind assembly` when the target's normal packaging should be applied.

## Disk images and LSP

```sh
ezrac disk [--format <format>] [--label <label>] --output <image> [--file [NAME=]PATH]...
ezrac lsp
```

`disk` creates M35FD, FAT12, or D64 images. See [disk images](disk-images.md). `lsp` is available only when the CLI is built with the `lsp` Cargo feature and communicates over stdio.

## Optimization levels

The default `-O2` enables scalar cleanup, propagation, loop-invariant code motion, known-bits cleanup, function inlining, tail calls, tail recursion, and related passes. `-O0` still performs dead-code elimination unless that pass is explicitly disabled. Pass names and project configuration are documented in [Projects](projects.md).
