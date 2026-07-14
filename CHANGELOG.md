# Changelog

## Unreleased

## 0.1.26

- Added `[lsp] mode = "library"` for SDK and module projects that need language-server diagnostics without an executable `main` function.
- Added the public `ezra::api` in-process compiler API for compiling source strings to target assembly from Rust applications.
- Added high-level `ti99.graphics` and `ti99.sprites` helpers plus TMS9918A VDP register, transfer, fill, sprite, and timing primitives.
- Updated TI-99/4A Mandelbrot and atom examples to use the SDK helpers instead of duplicating VDP setup and sprite descriptor assembly.
- Declared the package as BSD-3-Clause, matching the repository license.

## 0.1.25

- Added the optional TMS9900 source backend and a `ti99-4a-tms9900` target that emits a bootable TI-99/4A cartridge ROM.
- Expanded the TMS9900 assembler, corrected dual-operand encodings, and added Libre99 CPU-backed assembler and source-codegen tests.
- Added bundled `ti99.*` SDK modules plus TI-99/4A Mandelbrot tile-study and atom-sprite-animation examples.


## 0.1.24

- Added a tokenized `10 SYS2061` BASIC autostart loader to Commodore 64 `.prg` output so VICE launches C64 programs automatically.
- Added `output = "crt"` for autostarting standard 8 KiB C64 CRT cartridges.
- Eliminated unreachable functions from executable output regardless of their visibility, substantially reducing imported SDK code.
- Added C64 keyboard polling through `cia.key_pressed(key)` and made C64 programs return to BASIC after `main` exits.

## 0.1.23

- Added the `commodore64-6502` target with C64 `.prg` output, a 16-bit C64 memory layout, MOS 6502 source code generation, and bundled `c64.vic`, `c64.sid`, `c64.cia`, `c64.memory`, and `c64.text` SDK modules.
- Added target-aware platform text SDK helpers for supported systems.

## 0.1.22

- Added the feature-gated `generic-m68k-bare` source target with a 24-bit layout and raw binary output.
- Added M68k lowering for scalar and 24-bit values, pointers, arrays, structs, strings, memory helpers, control flow, calls, and inline assembly.
- Expanded the M68000 assembler with register XOR and arithmetic/logical shift instructions.

## 0.1.21

- Added standalone assemblers for MOS 6502, M6800, M68000, and AVR targets.
- Added Game Boy Color palette, input, and scrolling SDK helpers plus an interactive color example.
- Expanded the CP/M BDOS SDK to cover all CP/M 2.2 system calls.
- Gated processor families behind Cargo features; Intel, Z80-family, and LR35902 support remain enabled by default.

## 0.1.20

- Added direct EZRA source builds for DMG `.gb` and Game Boy Color `.gbc` ROMs, with LR35902 emulator-backed tests and complete assembler verification.
- Added built-in `gb.video`, `gb.sprites`, `gb.input`, `gb.audio`, and `gb.serial` source SDK modules for PPU setup, tiles, hardware sprites, controls, sound, wave tables, and serial output.
- Added assembly-free Game Boy background, sprite, serial, and input/audio examples validated visually with mGBA.
- Added portable project-level asset placement rules with target-pattern overrides for section and alignment, while preserving explicit source-level embed settings.
- Added compile-time SDK argument lowering for embedded asset addresses and constants, enabling calls such as `sprites.upload_tile1(&player)` and `audio.load_wave(&wave)`.
- Upgraded the `ez80` emulator dependency to 0.5.0 for native Game Boy LR35902 CPU mode.

## 0.1.19

- Initially added separate `gameboy-dmg-lr35902` and `gameboy-color-lr35902` assembly targets with valid ROM-only `.gb` packaging and checksums; these targets now also support EZRA source compilation.
- Added complete documented LR35902 base and CB opcode assembly coverage without accepting unsupported Z80 instructions.
- Added a vendorable Game Boy assembly macro SDK for common DMG and CGB hardware programming idioms.

## 0.1.18

- Added target-independent handwritten-assembly preprocessing with vendorable includes, defines, CPU/target conditionals, parameterized macros, and hygienic macro-local labels.
- Applied preprocessing consistently to `assemble --base` and assembly builds.
- Added an extensible test-runner backend interface, including eZ80 crate execution for eZ80, Z80-family, i8080, and i8085 targets.
- Expanded parser-derived semantic diagnostic targeting and generated eZ80 assembler coverage.

## 0.1.17

- Added ez180N console frame-tick synchronization through `console.frame_tick()` and `console.wait_tick()`.

## 0.1.16

- Added statement-scoped semantic diagnostics so independent errors in function bodies can be reported with useful source locations.
- Added default ZX Spectrum `.tap` output, tape validation coverage, and a loadable Spectrum hello-world example.
- Added safe TBIR expression optimization with boolean-literal folding, algebraic identity rewrites, optimization reporting, and validation-preserving dead-statement markers.

## 0.1.15

- Preserved imported source provenance through resolution so semantic diagnostics and multiple unknown-reference errors point into the correct module.

## 0.1.14

- Added LSP go-to-definition across local and imported declarations, document/workspace symbols, semantic tokens, watched-file registration, and project diagnostics that honor unsaved imports.
- Expanded LSP completion for struct fields, cfg predicates and target values, exposed layout symbols to completion/hover, and made bundled SDK definitions navigable as real cached source files.

## 0.1.13

- Fixed eZ80 indexed addressing at the full signed displacement boundary, including `-128`.
- Fixed two-pass assembly of mode-suffixed instructions with label operands.
- Kept imported SDK member completion available while the document contains incomplete syntax.
- Fixed nested-call signature help and UTF-16 LSP position/range handling.
- Fixed the ez180N Meteor Runner example after the console button API changed to `bool`.

## 0.1.12

- Keep LSP completion available for recoverable local symbols while an in-progress edit leaves an `if` or `while` statement syntactically incomplete.

## 0.1.11

- Added LSP signature help with active parameter tracking and argument-list completion triggers.
- Report LSP diagnostics on relevant source ranges when compiler diagnostics lack precise locations.
- Made SDK import completion target-aware and fixed ez180N console button results to use `bool`.

## 0.1.7

- Added the `ez180n-ez80` `.gaem` output target layout for out-of-the-box ez180N fantasy console cartridges.
- Added an ez180N Meteor Runner example and updated ez180N examples for the 80x56 framebuffer.
- Documented the ez180N libretro console target and SDK usage.

- Added `cpm.fcb` and `cpm.dma` SDK modules plus a CP/M FCB/DMA source example.
- Added a CP/M SDK tracker, a CP/M source example, richer `cpm.console` helpers, and BDOS 9 VM test support.
- Added `ezrac targets` to list documented target triples, default outputs, SDK families, and support status.
- Added an Agon app guide for console apps, games/visualizations, sprite games, and graphical apps.
- Added small `agon.vdp` convenience helpers for mode 8 setup, drawing color selection, graphics clearing, and simple frame delays.
- Deduplicated `ezrac lsp` completion labels and reduced noisy SDK completions outside import statements.
- Improved `ezrac lsp` completions, hover information, and unknown-symbol diagnostics.
- Updated Cargo dependencies, including `toml` 1.1.x compatibility for project files.
- Expanded `agon.vdp` with cursor helpers, bright color constants, line helpers, filled/framed rectangles, and triangles.
- Added optional `ezrac lsp` support behind the `lsp` Cargo feature.
- Added editor LSP integration docs and launcher support for Helix, micro, Vim/Neovim, VS Code, and Zed.
- Added the `agonlight-mos-ez80` target profile for Agon Light MOS programs.
- Added bundled Agon SDK modules under `toolchains/agonlight-mos-ez80/sdk`.
- Emitted Agon MOS executable wrappers with the MOS header at byte `64` and program entry at `0x040045`.
- Updated the Agon MOS runtime path to preserve the MOS stack, enable interrupts, use `rst.lis`, and return to MOS after `main`.
- Expanded eZ80 assembler coverage for common control-flow, ALU, register, indexed, I/O, and block-operation mnemonics.
- Added `ezra assemble` for standalone eZ80 assembly to raw binary output.
- Routed build artifacts through project-local `target/<target>/...` directories.
- Added `[build].executable` in `Ezra.toml` to control artifact basenames.
- Added Agon MOS examples, including an interactive coffee-order demo.
