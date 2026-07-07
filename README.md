# EZRA Compiler

`ezrac` is an experimental compiler and tooling prototype for the EZRA language.

EZRA is a small compiled language for eZ80 ADL-mode game cartridges. It is designed around explicit integer sizes, 24-bit addressing, direct memory and port I/O, embedded assets, inline assembly, readable generated assembly, and emulator-backed tests.

This is alpha software. The language and cartridge format are still evolving, and not every part of `spec.md` is implemented yet.

## Commands

```sh
cargo run -- check <file.ezra>
cargo run -- emit-asm <file.ezra>
cargo run -- build <file.ezra>
cargo run -- test <file.ezra>
cargo run -- assemble [--base <addr>] [--output <file.bin>] <file.asm>
cargo run -- layout
cargo run -- header
```

`build` writes `.asm`, `.map`, and a target executable under a Rust-like `target` directory. If the source belongs to a project with `Ezra.toml`, artifacts go under `<project>/target/<target>/...`. Otherwise they go under a `target` directory next to the source. The default executable format is raw `.bin`.

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

The `agonlight-mos-ez80` target emits eZ80 ADL-mode programs for Agon MOS. It uses the built-in SDK under `toolchains/agonlight-mos-ez80/sdk`, including `agon.mos` wrappers for MOS character output, string output, blocking key reads, and keyboard-state clearing.

MOS executable builds use the documented Agon format:

- byte `0`: `JP 0x040045`
- byte `64`: `"MOS", 0, 1`
- byte `69`: compiled program code
- default entry address: `0x040045`

The runtime preserves the MOS stack, enables interrupts for MOS/VDP interaction, calls `main`, and returns to MOS when `main` returns. Normal MOS programs should return rather than writing emulator-only exit ports.

Examples live under `examples/agon-mos`. See `examples/agon-mos/README.md` for build and Fab Agon Emulator usage.

## Project Notes

- `spec.md` describes the intended language, runtime, and cartridge format.
- `CHANGELOG.md` summarizes notable development milestones.
- `REMAINING_WORK.md` tracks known gaps and follow-up work.
- `docs/ez80-opcode-coverage.md` tracks assembler opcode coverage and roadmap items.
- The current implemented backend is eZ80 ADL mode. Classic Z80 support is planned as separate future work.
- Bundled target SDKs are EZRA source files under `toolchains/*/sdk` and are embedded into the compiler binary.
- Agon Light MOS examples live under `examples/agon-mos`.
- Fab Agon Emulator is GPL-3.0 and is not vendored. Use `FAB_AGON_EMULATOR_DIR` with `tools/run-fab-agon.ps1` to point at a local checkout or release.

## Development

```sh
cargo fmt
cargo test --quiet
git diff --check
```
