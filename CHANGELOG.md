# Changelog

## Unreleased

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
