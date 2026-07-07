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
cargo run -- layout
cargo run -- header
```

`build` writes `.asm`, `.map`, and a target executable next to the source file. The default executable format is raw `.bin`.

## Project Notes

- `spec.md` describes the intended language, runtime, and cartridge format.
- `REMAINING_WORK.md` tracks known gaps and follow-up work.
- The current target is eZ80 ADL mode. Classic Z80 support is planned as separate future work.

## Development

```sh
cargo fmt
cargo test --quiet
git diff --check
```
