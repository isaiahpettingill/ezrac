# EZRA Compiler

`ezrac` is an experimental compiler and tooling prototype for the EZRA language.

EZRA is a small compiled language for explicit, low-level game and hobby-computer targets. It is designed around explicit integer sizes, target-defined address widths, direct memory and port I/O, embedded assets, inline assembly, readable generated assembly, and emulator-backed tests.

This is alpha software. The language, target profiles, and cartridge formats are still evolving. Use `docs/language.md`, `docs/usage.md`, and `docs/platforms.md` for current implemented behavior; `spec.md` is the broader design document.

Implementation status for every specification section is tracked in [`SPEC_COVERAGE.md`](SPEC_COVERAGE.md).

## Commands

After installation, run commands with `ezrac`:

```sh
ezrac check <file.ezra>
ezrac emit-asm <file.ezra>
ezrac emit-ir [--stage hir|tbir] <file.ezra>
ezrac build <file.ezra>
ezrac disk --format <format> --output <image> [--file [NAME=]PATH]...
ezrac test [<file.ezra>]
ezrac assemble [--base <addr>] [--output <file.bin>] <file.asm>
ezrac init [--name <name>] [--target <triple>] [dir]
ezrac install-syntax (--all | [--editor] <editor>...)
ezrac targets
ezrac lsp
ezrac layout
ezrac header
```

For local development, use Cargo:

```sh
cargo run -- check <file.ezra>
cargo run -- emit-asm <file.ezra>
cargo run -- emit-ir [--stage hir|tbir] <file.ezra>
cargo run -- build <file.ezra>
cargo run -- disk --format <format> --output <image> [--file [NAME=]PATH]...
cargo run -- test [<file.ezra>]
cargo run -- assemble [--base <addr>] [--output <file.bin>] <file.asm>
cargo run -- init [--name <name>] [--target <triple>] [dir]
cargo run -- install-syntax (--all | [--editor] <editor>...)
cargo run -- targets
cargo run --features lsp -- lsp
cargo run -- layout
cargo run -- header
```

`build` writes `.asm`, `.map`, and a target executable under a Rust-like `target` directory. If the source belongs to a project with `Ezra.toml`, artifacts go under `<project>/target/<target>/...`. Otherwise they go under a `target` directory next to the source. Output formats include raw `.bin`, NES `.nes`, CP/M and MS-DOS `.com`, Intel HEX, ZX Spectrum tape, Game Boy ROM, Commodore 64 PRG, and TI calculator formats; see `docs/usage.md`.

`disk` creates M35FD images for DCPU-16, FAT12 floppy images for CP/M through IS-DOS, MOS, and DOS, and D64 images for C64. Each image can contain multiple named files. See `docs/disk-images.md`.

`init` creates a non-destructive starter project with `.gitignore`, `Ezra.toml`, `README.md`, `src/main.ezra`, `sdk/`, and `assets/`. `install-syntax` installs syntax files for selected editors; supported editor names are `vim`, `neovim`, `nano`, `micro`, `helix`, `vscode`, `zed`, and `notepad++`.

`lsp` starts the EZRA language server over stdio. It is behind the optional Cargo feature `lsp`, so default installs do not include LSP dependencies. Build or install with `--features lsp` to enable it. Editor setup notes live in `docs/editor-syntax.md`.

## Project Files

EZRA projects use `Ezra.toml`. All fields are optional unless a target-specific feature needs them.

```toml
[project]
name = "my-program"

[build]
target = "agonlight-mos-ez80"
output = "bin"
executable = "my-program"

[test]
target = "ezra-test-flat-ez80"

[optimization]
level = 2
disable = ["function-inlining"]

[layout]
file = "layouts/custom.ezralayout"

[sdk]
paths = ["sdk"]

[lsp]
mode = "application" # or "library" for an importable SDK/module project
```

- `[build].target` selects the target profile. `agonlight-mos-ez80` builds a normal Agon MOS executable.
- `[build].output` selects the executable format. The current default is raw `bin`; cartridge layouts are explicit configuration.
- `[build].executable` overrides the artifact basename. Without it, the source file stem is used.
- `[test].target` selects the target used by project test discovery. `ezrac test` discovers `tests/**/*.ezra` in deterministic path order, builds artifacts under `target/<target>/`, and runs each test. CLI `--target` overrides `[test].target`, which overrides `[build].target`.
- `[optimization].level` selects `0` through `3`; the default is `2`. `enable` and `disable` contain named pass overrides. Dead-code elimination remains enabled at level `0` unless `dead-code-elimination` is explicitly disabled. Source commands also accept `-O0` through `-O3`, `--enable-optimization <pass>`, and `--disable-optimization <pass>`.
- `[layout].file` points at a custom layout file.
- `[sdk].paths` adds project SDK source roots in addition to bundled target SDKs.
- `[lsp].mode = "library"` checks the configured source and imports as a library module without requiring `fn main()`. Library mode supports LSP diagnostics and SDK imports, but `build` still creates executables only.

## Implemented language features

The compiler includes built-in `ezra.bits`, `ezra.int`, and `ezra.mem` intrinsic catalogs. They cover width-aware bit operations, defined integer helpers, overlap-aware memory operations, explicit-endian loads/stores, and scalar byte access. Functions may have zero, one, or two ordered primitive results using `-> T, U`, two-place `let`, and `return a, b`; this is not tuple or large-value return support. Exact widths, pointer widths, and result ABIs are target-specific, and unsupported combinations diagnose. See [`docs/language.md`](docs/language.md) for the complete catalog and restrictions.

## Agon Light MOS

The `agonlight-mos-ez80` target emits eZ80 ADL-mode programs for Agon MOS. It uses the built-in SDK under `toolchains/agonlight-mos-ez80/sdk`, including `agon.mos` wrappers for MOS character output, string output, blocking key reads, and keyboard-state clearing, plus `agon.console` convenience wrappers for console-style output.

MOS executable builds use the documented Agon format:

- byte `0`: `JP 0x040045`
- byte `64`: `"MOS", 0, 1`
- byte `69`: compiled program code
- default entry address: `0x040045`

The runtime preserves the MOS stack, enables interrupts for MOS/VDP interaction, calls `main`, and returns to MOS when `main` returns. Normal MOS programs should return rather than writing emulator-only exit ports.

Examples live under `examples/agon-mos`. See `docs/agon-apps.md` for app patterns and `examples/agon-mos/README.md` for build and Fab Agon Emulator usage.

## Project Notes

- `docs/language.md` documents the currently implemented EZRA source language.
- `docs/usage.md` documents compiler commands, project files, outputs, layouts, and SDK imports.
- `docs/platforms.md` documents supported target profiles and platform-specific coding guidance.
- `docs/agon-apps.md` explains how to write Agon console apps, games/visualizations, and graphical apps.
- `docs/gameboy-assembly.md` documents DMG/CGB LR35902 assembly, ROM output, and the vendorable macro SDK.
- `examples/nes-2a03/source-hello` compiles EZRA source into an NROM-128 NES image.
- `examples/nes-2a03/hello-world` contains the raw 2A03 NES hello-world assembly example.
- `examples/snes-5a22/source-hello` builds EZRA source with the bundled SNES SDK into a LoROM `.sfc` image.
- `examples/snes-5a22/hello-world` contains a raw Ricoh 5A22/65C816 assembly example.
- `docs/i8086-assembly.md` documents the optional complete strict Intel 8086 standalone assembler and source backend.
- `docs/disk-images.md` documents the disk-image command, emulator profiles, and `no_std + alloc` API.
- `docs/dcpu-assembly.md` documents the optional DCPU-16 1.7 assembler, limited source backend, operand forms, expressions, word data, and [`examples/dcpu-16`](examples/dcpu-16/).
- [`dcpu-16-libretro`](https://github.com/isaiahpettingill/dcpu-16-libretro) provides the DCPU-16 Standard Machine libretro core; its published [`dcpu16-core`](https://crates.io/crates/dcpu16-core) crate backs EZRAC's DCPU emulator tests.
- `docs/msdos-sdk.md` documents the `msdos-com-i8086` `.COM` target and bundled `dos.*` SDK.
- `docs/cpm-sdk-tracker.md` tracks CP/M SDK coverage and remaining work.
- `spec.md` describes the intended language, runtime, and cartridge format.
- `docs/editor-syntax.md` describes EZRA syntax-highlighting files for supported editors.
- `docs/image-assets.md` describes indexed PNG conversion to native sprite, tile, and bitmap formats.
- `docs/real-core-tests.md` explains how to run the opt-in `play96` example suites against real libretro cores.
- `docs/real-core-test-results.md` publishes the latest reviewed core identities and pass results.
- `CHANGELOG.md` summarizes notable development milestones.
- `docs/ez80-opcode-coverage.md` tracks assembler opcode coverage and roadmap items.
- The main source target is Agon Light MOS on eZ80 ADL. Default builds include every compiler backend: Intel 8080/8085/8086, eZ80/Z80-family, LR35902, AVR, MOS 6502-family, M6800/M6809, M68k, TMS9900, and DCPU-16. `ti99-4a-tms9900` emits a bootable one-bank TI-99/4A cartridge ROM with the bundled `ti99.*` SDK. The i8086 backend provides scalar code generation, recursion, aggregate storage, constrained interrupt handlers, typed inline assembly, a complete strict 8086 assembler, and the `msdos-com-i8086` target with bundled `dos.*` SDK; aggregate parameters and returns must be passed by pointer. Target profiles remain at varying maturity levels; see `docs/platforms.md`.
- Bundled target SDKs are EZRA source files under `toolchains/*/sdk` and are embedded into the compiler binary.
- Agon Light MOS examples live under `examples/agon-mos`.
- Fab Agon Emulator is GPL-3.0 and is not vendored. Use `FAB_AGON_EMULATOR_DIR` with `tools/run-fab-agon.ps1` to point at a local checkout or release.

## Embedding the Compiler

The `ezra-core` package exposes the `ezra` library crate with filesystem-free compile and build APIs. The `ezrac-cli` workspace package contains the `ezrac` binary and LSP server. A virtual workspace can be compiled, assembled, and packaged without invoking the CLI or writing artifacts:

```rust
use ezra::api::{BuildCompilation, CompileRequest, Workspace, WorkspaceFile, build_workspace};

let files = [
    WorkspaceFile::text(
        "src/main.ezra",
        "import math\nfn main() { let answer: u8 = math.ANSWER }",
    ),
    WorkspaceFile::text("src/math.ezra", "pub const ANSWER: u8 = 42"),
];
let request = CompileRequest::new("src/main.ezra", "cpm-2.2-z80");
let build: BuildCompilation =
    build_workspace(&Workspace::new(&files), "src/main.ezra", &request)?;

assert_eq!(build.executable_extension, "com");
// build.assembly, build.machine_code, build.symbols, and build.executable
// are all caller-owned in-memory artifacts.
```

`build_workspace` resolves imports from supplied files and returns target assembly, machine code, symbols, and native Agon MOS, CP/M, C64, raw, or Intel HEX package bytes. For explicit layouts, output formats, package metadata, section-aware standalone assembly, or explicit-base flat assembly, use `BuildRequest`, `build_workspace_with_request`, `link_generated_assembly`, `link_assembly_program`, or `link_assembly_program_at` from `ezra::api`. Explicit layouts drive both source code generation and final linking. The CLI `build` and `assemble` commands resolve host configuration and files into these same library pipelines; they do not own separate compiler, linker, or packager implementations.

`ezra::api`, `diagnostic`, `disk`, `image`, `layout`, `package`, `parser`, and `target` are the supported embedding surface. The crate remains pre-1.0, so breaking API changes may occur in minor releases; documented public types and functions follow semantic versioning once 1.0 is released. Other public modules expose compiler implementation details and should be treated as unstable.

Both std and alloc-only builds validate the selected layout, strictly validate generated target assembly, and ensure the assembled `.text` bytes fit the region assigned by the layout before packaging. Source parsing, import resolution, code generation, assembly, linking, maps, explicit layouts, and packaging work under `no_std + alloc` for every compiler backend. Select only the backends an embedded consumer needs:

```sh
cargo check -p ezra-core --lib --no-default-features --features no-std,z80
cargo check -p ezra-core --lib --no-default-features --features no-std,mos6502
cargo check -p ezra-core --lib --no-default-features --features no-std,i8086
cargo check -p ezra-core --lib --no-default-features --features no-std,avr,m6800,m6809,m68k,tms9900,dcpu,lr35902
```

No-std builds never access host paths: all imported SDK source and binary assets must be included in `Workspace`. The full in-memory indexed PNG pipeline works with `no_std + alloc` through `ezra::image::decode_indexed_png` and `indexed_png_to_native_bytes`; embedded callers supply the PNG bytes and then add the converted bytes to their workspace. In virtual builds, `embed file("assets/blob.bin")` resolves relative to the Ezra source file that declares it and reads the matching `WorkspaceFile`; this also works for assets declared by imported modules. Inline byte, text, C-string, and repeat embeds remain available. The library is checked for `wasm32-unknown-unknown` in both no-std configurations without `wasm-bindgen`. Filesystem project discovery, the CLI, LSP, and emulator test runner remain behind `std`; the external MOS 6502 emulator is separately opt-in through `mos6502-emulator`.

## Development

The root is a Cargo workspace with two packages:

- `ezra-core` at the repository root contains the compiler library and supports `no_std + alloc`.
- `ezrac-cli` under `crates/ezrac-cli` contains the host CLI, LSP server, and editor installer.

Root `cargo run -- <args>` commands still run the `ezrac` binary. Install it from a checkout with:

```sh
cargo install --path crates/ezrac-cli --features lsp
```

```sh
cargo fmt
cargo test --quiet
git diff --check
```

Real-core example tests are ignored by default because they require third-party libretro shared libraries. See [`docs/real-core-tests.md`](docs/real-core-tests.md) for setup and commands.
