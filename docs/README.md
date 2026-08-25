# EZRA documentation

EZRA is an experimental compiled language and toolchain for low-level games and hobby-computer targets. This documentation describes the implementation in this repository, not every feature proposed in [`spec.md`](spec.md).

## Start here

- [Getting started](getting-started.md) — install or build the compiler and make a first build.
- [CLI reference](cli.md) — commands, options, outputs, and test execution.
- [Projects and `Ezra.toml`](projects.md) — project discovery, configuration, SDK paths, and artifacts.
- [Targets and layouts](targets-and-layouts.md) — target triples, support levels, memory layouts, and output formats.
- [Assembly](assembly.md) — handwritten assembly, preprocessing, `assemble`, and current opcode limits.
- [Built-in SDKs](sdk.md) — how target SDK modules are found and imported.
- [Examples index](examples.md) — repository examples grouped by target and purpose.
- [Diagnostics and troubleshooting](diagnostics-and-troubleshooting.md) — common failures and how to narrow them down.

## Language reference

- [Language overview](language/README.md)
- [Modules, imports, and visibility](language/modules-imports.md)
- [Types, literals, and casts](language/types.md)
- [Constants and compile-time evaluation](language/constants.md)
- [Globals, ports, and MMIO](language/globals.md)
- [Functions and modifiers](language/functions.md)
- [Control flow and expressions](language/control-flow.md)
- [Pointers and pointer casts](language/pointers.md)
- [Arrays and indexing](language/arrays.md)
- [Structs and field access](language/structs.md)
- [Inline assembly](language/inline-asm.md)
- [Conditional compilation and banking syntax](language/conditional-compilation.md)
- [Embedded data and image assets](language/embeds-assets.md)
- [Built-in test, debug, and memory helpers](language/diagnostics.md)

The older [single-page language reference](language.md) remains available for readers who want the full reference in one file.

## Tutorials

- [Hello and a test](tutorials/hello-test.md)
- [Agon MOS coffee-order program](tutorials/agon-mos-coffee-order.md)
- [Assembly-only program](tutorials/assembly-only-program.md)
- [Custom layout](tutorials/custom-layout.md)

## Contributor notes

- [Compiler pipeline](internals/compiler-pipeline.md)
- [HIR and TBIR](internals/hir-tbir.md)
- [Assembler internals](internals/assembler.md)
- [Target and layout internals](internals/targets.md)
- [Language and runtime specification](spec.md) — design goals and proposed behavior.
- [Specification coverage](spec-coverage.md) — implementation and test status by specification section.
- [Remaining work](remaining-work.md) — current engineering backlog.
- [Optimization design notes](internals/optimizations.md) — candidate and implemented optimization work.
- [Optimization safety review](internals/optimization-safety.md) — side-effect and hardware-safety rules.

## Existing platform and tool guides

These pages cover target-specific features, emulator checks, and older focused investigations:

- [Platforms and support levels](platforms.md)
- [Compiler usage (legacy single-page guide)](usage.md)
- [Agon applications](agon-apps.md)
- [Agon Light assembly audit](agon-light-assembly-audit.md)
- [6502 opcode coverage](6502-opcode-coverage.md)
- [65C816 assembly](65c816-assembly.md)
- [DCPU-16 assembly](dcpu-assembly.md)
- [Disk images](disk-images.md)
- [Editor syntax](editor-syntax.md)
- [eZ80 opcode coverage](ez80-opcode-coverage.md)
- [eZ80 test harness targets](ez80-test-harness-targets.md)
- [FreeDOS compiler](freedos.md)
- [eZ80/Z80 source ABI](ez80-source-abi.md)
- [Game Boy assembly](gameboy-assembly.md)
- [Image assets](image-assets.md)
- [IR design notes](ir-design.md)
- [M6800 assembly](m6800-assembly.md)
- [M6809 assembly](m6809-assembly.md)
- [M68k assembly](m68k-assembly.md)
- [MOS 6502 assembly](mos6502-assembly.md)
- [MS-DOS SDK](msdos-sdk.md)
- [PIC18 assembly](pic18-assembly.md)
- [R800 assembly](r800-assembly.md)
- [Real-core test setup](real-core-tests.md)
- [Published real-core results](real-core-test-results.md)
- [TMS9900 assembly](tms9900-assembly.md)

Target-specific pages:

- [Commodore 64](targets/commodore64.md)
- [NES](targets/nes.md)
- [Sega Game Gear](targets/sega-game-gear.md)
- [Sega Master System](targets/sega-master-system.md)
- [SNES](targets/snes.md)

## Project status

EZRA is pre-1.0 alpha software. Target ABIs, SDK APIs, package formats, and some language features can change. [Specification coverage](spec-coverage.md) tracks the broader design, while [remaining work](remaining-work.md) lists known unfinished work.
