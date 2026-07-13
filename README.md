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
ezrac test <file.ezra>
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
cargo run -- test <file.ezra>
cargo run -- assemble [--base <addr>] [--output <file.bin>] <file.asm>
cargo run -- init [--name <name>] [--target <triple>] [dir]
cargo run -- install-syntax (--all | [--editor] <editor>...)
cargo run -- targets
cargo run --features lsp -- lsp
cargo run -- layout
cargo run -- header
```

`build` writes `.asm`, `.map`, and a target executable under a Rust-like `target` directory. If the source belongs to a project with `Ezra.toml`, artifacts go under `<project>/target/<target>/...`. Otherwise they go under a `target` directory next to the source. Output formats include raw `.bin`, CP/M `.com`, Intel HEX, ZX Spectrum tape, Game Boy ROM, Commodore 64 PRG, and TI calculator formats; see `docs/usage.md`.

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

[layout]
file = "layouts/custom.ezralayout"

[sdk]
paths = ["sdk"]
```

- `[build].target` selects the target profile. `agonlight-mos-ez80` builds a normal Agon MOS executable.
- `[build].output` selects the executable format. The current default is raw `bin`; cartridge layouts are explicit configuration.
- `[build].executable` overrides the artifact basename. Without it, the source file stem is used.
- `[layout].file` points at a custom layout file.
- `[sdk].paths` adds project SDK source roots in addition to bundled target SDKs.

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
- `docs/cpm-sdk-tracker.md` tracks CP/M SDK coverage and remaining work.
- `spec.md` describes the intended language, runtime, and cartridge format.
- `docs/editor-syntax.md` describes EZRA syntax-highlighting files for supported editors.
- `docs/real-core-tests.md` explains how to run the opt-in `play96` example suites against real libretro cores.
- `docs/real-core-test-results.md` publishes the latest reviewed core identities and pass results.
- `CHANGELOG.md` summarizes notable development milestones.
- `REMAINING_WORK.md` tracks known gaps and follow-up work.
- `docs/ez80-opcode-coverage.md` tracks assembler opcode coverage and roadmap items.
- The main source target is Agon Light MOS on eZ80 ADL. Additional eZ80, Z80-family, 8080-family, TI, ZX Spectrum, CP/M, and bare profiles exist at varying maturity levels; see `docs/platforms.md`.
- Bundled target SDKs are EZRA source files under `toolchains/*/sdk` and are embedded into the compiler binary.
- Agon Light MOS examples live under `examples/agon-mos`.
- Fab Agon Emulator is GPL-3.0 and is not vendored. Use `FAB_AGON_EMULATOR_DIR` with `tools/run-fab-agon.ps1` to point at a local checkout or release.

## Development

```sh
cargo fmt
cargo test --quiet
git diff --check
```

Real-core example tests are ignored by default because they require third-party libretro shared libraries. See [`docs/real-core-tests.md`](docs/real-core-tests.md) for setup and commands.
