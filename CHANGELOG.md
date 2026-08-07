# Changelog

## 0.1.34

- Added a shared indexed-image encoder and CLI pipeline that converts configured indexed PNG assets into native Game Boy, NES, Sega, C64, TI, ZX Spectrum, Arduboy, and Agon sprite, tile, or bitmap bytes.
- Added built-in `ezra.bits`, `ezra.int`, and `ezra.mem` intrinsic catalogs with width-aware bit operations, defined integer helpers, overlap-aware memory operations, explicit-endian access, and volatile/MMIO restrictions.
- Added zero-, one-, and two-result function syntax (`-> T, U`, two-place `let`, and `return a, b`) without introducing tuples or aggregate returns. Target-specific widths and ABIs reject unsupported combinations.
- Added `-O0` through `-O3`, named optimization-pass overrides in the CLI, `Ezra.toml`, and library API, plus safe idempotent bit-operation cleanup, cross-target redundant register-copy removal, and an `-O3` second pass informed by MDL's pattern optimizer. Dead-code elimination remains enabled at `-O0` unless explicitly disabled.
- R800 code generation now uses `MULUB` and `MULUW` for equal-width widening, high-half, and full-result integer intrinsics as well as ordinary multiplication.

## 0.1.33

- Enabled every compiler backend by default and added `no_std + alloc` compilation for every CPU family, with a CI feature matrix covering each backend independently.
- Expanded the alloc-only embedding API with explicit build requests, layouts, linking, maps, standalone assembly, and filesystem-free packaging context.
- Fixed generic i8086 linking, AVR API dispatch, 65C816 pointer width, inactive conditional embeds, versioned MS-DOS targets, Intel HEX addressing, and unsupported target/output diagnostics.
- Fixed M6800 multiplication, i8086 and M68k regressions, MOS banked-declaration handling, Arduboy EEPROM interrupt restoration, and newline-separated dereference assignments around comments.
- Hardened project and CLI handling for artifact paths, same-stem outputs, layout-checked tests, symlink cycles, project discovery, scaffold preflight, case-insensitive extensions, non-UTF-8 paths, and nested outputs.
- Fixed LSP handling for layout documents, closed-file workspace symbols, and unsaved imported modules.
- Added `dcpu.*` Standard Machine constants and a vendorable DCPU assembly macro SDK for LEM1802, keyboard, clock, speaker, and generic `HWI` commands.
- Switched DCPU-16 emulator-backed compiler tests to the published `dcpu16-core` crate.
- Added handwritten LEM1802 and scalar-source DCPU-16 examples under `examples/dcpu-16`, build-tested as raw little-endian images for `dcpu-16-libretro`.
- Published the standalone [`dcpu-16-libretro`](https://github.com/isaiahpettingill/dcpu-16-libretro) Standard Machine core and its reusable CPU crate.

## 0.1.32

- Added an MS-DOS i8086 character-creation RPG example covering buffered name input, function pointers, mutable pointer output, loops, and DOS console helpers.
- Expanded the DCPU-16 assembler with DAS-compatible `DAT`/`.short` word data, `.set`, colon-prefixed labels, C-style string escapes, and constant expressions in literals and indexed operands. DCPU symbols and `$` now use word addresses throughout instruction, equate, and data evaluation while symbolic operands keep stable next-word encodings.
- Standardized tracked Rust crates on edition 2024 and set development to the latest nightly toolchain with Rustfmt and Clippy components.

- Moved explicit function inlining and safe direct-tail-recursion rewriting into TBIR so every source backend receives the same transformed program. Inline expansion preserves typed, left-to-right, exactly-once argument evaluation, renames parameters and locals, supports nested approved calls, and leaves conditionally evaluated calls in short-circuit and loop conditions unchanged. eZ80-family backends also lower compatible register-ABI sibling tail calls to jumps.
- Added explicit TBIR passes for immutable-local propagation, local common-subexpression cleanup, strength reduction, pure scalar loop-invariant hoisting, and named global-read LICM. Propagated values retain their declared width, signedness, and aliases. TBIR now retains typed global, MMIO, and embed object provenance with address, size, region, access, and volatility; calls, ports, MMIO, embeds, read-only or volatile regions, dereferences, addresses, inline assembly, unknown writes, and same-object writes remain conservative barriers.
- Added target instruction selection for common arithmetic: 8086 uses native `MUL`/`IMUL` and strict one-bit shift forms; AVR uses native signed/unsigned multiply and fixed shifts; M68000 uses `MULS.W`/`MULU.W` and immediate shifts; MC6800 uses `ASLA`/`LSRA`/`ASRA`; TMS9900 uses guarded `SLA`/`SRL`/`SRA`; and DCPU-16 selects signed `MLI`/`DVI`/`ASR`. MOS6502 and LR35902 use fixed shifts and shift/add sequences for small constants. Shared power-of-two multiplication strength reduction feeds shifts to every backend.
- Added a shared arithmetic edge-case corpus across all nine source backends, with runtime comparison on eZ80, MOS6502, TMS9900, and the supported MC6800 subset. The corpus covers wrapping, signed shifts, wide shift counts, signed and unsigned division/remainder including zero divisors, and power-of-two and non-power-of-two multiplication.
- Removed duplicated emitter-side explicit-inline policy. TBIR now owns explicit inlining across source backends; MOS6502 and TMS9900 retain only their separate unannotated compact-wrapper size optimization.


## 0.1.31

- Added generic Intel 8086 source code generation for scalar arithmetic and recursion, pointers, aggregate storage, control flow, memory and port I/O, constrained interrupt handlers, and typed inline assembly, plus alloc-only API support. Aggregate parameters and returns are explicitly rejected in favor of pass-by-pointer APIs.
- Added an optional complete Intel 8086 standalone assembler with strict 8086-only opcode/form validation, 16-bit ModR/M addressing, segment/repeat/lock prefixes, stable label fixups, a `bare-i8086` target, and golden coverage across the documented ISA.
- Hardened 8086 lowering with alias-aware signed operations and comparisons, one-time indirect-lvalue evaluation, MMIO access roots, constant bounds and return validation, typed ports, diagnostic allocation failures, interrupt scratch/register preservation and call isolation, AL/AX/memory/immediate inline-assembly operands, and unreachable-function elimination. Arbitrary resolvable i8086 triples now use a 16-bit generic layout, while CLI, std API, and alloc-only API consistently apply strict generated-assembly and `.text` region-fit validation.
- Added the first-class `msdos-com-i8086` target with PSP-aware `0100h` loading, DOS-provided stack preservation, `AH=4Ch` termination, raw `.COM` packaging, and a nine-module `dos.*` SDK covering console, files, directories, memory, date/time, processes, errors, constants, and PSP/FCB/JFT/environment access.
- Added MS-DOS hello, command-tail, and file-I/O examples plus a sixth `msdos-com-i8086` platform branch for TinyLisp. MZ `.EXE` packaging and deterministic DOS-emulator integration remain deferred.

## 0.1.30

- Added filesystem-free virtual workspace compilation and executable packaging APIs, including host-independent paths plus `alloc`-only, no-std, and Wasm library builds.
- Added the portable TinyLisp REPL for Agon Light, CP/M, Commodore 64, ZX Spectrum, and TI-99/4A, with translated keyboard input on the Spectrum and TI console-ROM KSCAN support.
- Reworked the TMS9900 ABI around stack frames and stack arguments, adding recursion, nested calls, strings, byte pointers, division/remainder, unreachable SDK elimination, and compact wrapper inlining.
- Reduced MOS 6502 output with bounded shared `u16` arithmetic helpers and recursion-aware caller preservation; added explicit `@inline` syntax and conservative cost-based wrapper inlining.
- Expanded TI calculator and Commodore 64 SDKs, examples, assembler coverage, packaging, and runtime validation.
- Audited the Game Boy assembly SDK against the GB ASM Tutorial and CC0 examples, adding safer HRAM OAM-DMA guidance, common LCD/STAT flags, RGBDS numeric literals and flag expressions, and background/sprite regression fixtures.

## 0.1.29

- Fixed `arduboy.input.read()` to expose and return its AVR inline-assembly result.
- Flattened project artifacts to `target/<triple>/<executable>.*`, removing the redundant source-directory component from every output path.
- Fixed AVR inline-assembly memory operand formatting and updated example documentation for the flattened paths.



## 0.1.28

- Added the playable Arduboy Snake example and real-core Arduous validation, with AVR pointer, carry/borrow, load-flag, and SDK import fixes.
- Added TMS9900 multiplication and embedded-asset lowering so TI-99/4A examples remain Ezra source, and build-validated both TI examples.
- Fixed Game Boy SDK argument handling, palette upload, input, video copying, and background-map layout; all six Game Boy examples now run on mGBA.
- Fixed CP/M BDOS/DMA multi-argument calls and deterministic lowercase IS-DOS launching; all source and assembly examples now execute visibly under ep128emu.
- Added complete real-core coverage for both ZX Spectrum and all four ez180N examples, including SDK bitmap-address correction and Mandelbrot captures.
- Added C64 hello and Mandelbrot runtime coverage for compatible libretro cores; current Windows Play96 combinations remain blocked by corrupt Frodo video and VICE native crashes.

## 0.1.27

- Added source lowering and test-runner emulator execution for M6800, TMS9900, and DCPU-16 programs.
- Added emulator-backed execution coverage for LR35902 Game Boy and M68000 programs.
- Completed shared Z80-family instruction metadata and eZ80 `LEA IY` operand forms.
- Expanded M6800 assembly with `JSR` forms and integrated the production M6800 backend.
- Added 65C02, 65C816, and Ricoh 2A03 MOS 6502 assembler variants alongside prior TI-99/4A, in-process compiler API, and library-mode LSP support.

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
