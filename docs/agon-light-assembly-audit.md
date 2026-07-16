# Agon Light SDK and example audit

Audited against [`schur/Agon-Light-Assembly`](https://github.com/schur/Agon-Light-Assembly), cloned from its `main` branch on 2026-07-16. That project is a small, `spasm-ng`-syntax collection last focused on the original MOS command ABI; it is useful compatibility input, but is not a complete or current Agon SDK reference.

## Coverage comparison

| Reference pattern | ezrac status | Notes |
| --- | --- | --- |
| MOS executable marker at byte 64 | Covered | `agonlight-mos-ez80` emits `MOS\0\1` with an ADL entry jump. |
| ADL programs loaded at `0x040000` | Covered | The Agon layout loads at `0x040000` and enters at `0x040045`. |
| Character and zero-terminated string output | Covered | `agon.mos.putc` / `puts`, `agon.console.write` / `print`, and `agon.text.print_ascii` cover the reference `PRT_CHR` / `PRSTR` workflows. |
| Hex output from `hello_world` | Not a dedicated API | The reference supplies `Print_Hex8/16/24`; applications can implement formatting with `agon.text`, but the SDK does not yet provide numeric formatting helpers. |
| Command-line argument scanning from `checkargs` / `memory_dump` | Not exposed | The reference reads MOS's incoming command-line pointer. ezrac currently builds ADL executables and does not expose process arguments to `main`. |
| Mixed Z80/SPS and ADL/SPL stack demo | Deliberately out of scope | ezrac's Agon target is ADL-only. The reference's `.LIS`, `.SIS`, and MOS-command stack-repair sequence targets legacy 16-bit MOS commands. |
| Current graphical/input APIs | ezrac extends it | `agon.vdp`, `buffers`, `sprites`, `keyboard`, `mouse`, and `gpio` go beyond the reference collection. |

## Example suite assessment

The bundled examples exercise the SDK at increasing scope:

- `hello` and `console`: VDU output and screen control.
- `coffee-order`: MOS string output, keyboard state clearing, and blocking key input.
- `mandelbrot`: repeated plotting and integer arithmetic.
- `sdk-showcase`: console, VDP, keyboard, mouse, GPIO, and VDP buffers.
- `space-invaders`: sprites, input, and an interactive game loop.

This is broader hardware coverage than the reference examples. The missing equivalence is command-line utility support and reusable hex formatting, not graphics or basic MOS/VDU output.

## Assembler regression coverage

`src/asm/ez80/tests.rs` contains a reference-derived regression using instruction forms from the reference `hello_world`, `extest`, `stacktest`, and startup code. It verifies `.L` as the `spasm-ng` alias for `.LIL`, plus `.LIS`, `.SIS`, and `RST.LIL` encodings.

The project does **not** ingest the reference `.asm` files directly: they depend on `spasm-ng` directives/macros (`#include`, `.DB`, `.BLOCK`, conditional assembly) and a legacy Z80 command startup ABI that EZRA intentionally does not model. The tests instead cover the eZ80 instruction dialect shared by those examples.

## `AgonPlatform/agon-ez80asm` bootstrap audit

`agon-ez80asm` at commit `5dc733c286b7864e3eb05ef93462c7e1637ba51e` is implemented primarily in C and built by the AgDev C toolchain. EZRA cannot compile that C implementation. Its handwritten assembly consists of the AgDev ABI shims `src/getfilesize.asm` and `src/removefile.asm`; its `tests/**/*.s` files are the relevant eZ80 source-compatibility corpus.

EZRA's standalone assembler accepts the shims' `.assume adl=1`, `.section`, `.global`, uppercase mnemonic/register, tabbed-whitespace, and `RST.LIL` syntax. It also encodes the required ADL 24-bit pointer loads, verified with the upstream native assembler:

- `ld hl, (ix+d)` → `DD 27 d`
- `ld hl, (hl)` → `ED 27`

Both handwritten AgDev shim files now assemble successfully through `ezrac assemble`. `.assume adl=0` is rejected because the standalone assembler does not implement legacy 16-bit mode switching safely.

## Follow-up candidates

1. Promote both AgDev shim files to CLI golden tests and expand the standalone parser against `agon-ez80asm/tests`, starting with `Opcodes` and `Addressing`; macros, expression grammar, relocation, and conditional assembly remain larger separate work items.
2. Design a stable MOS process-argument API before supporting the legacy command-line examples; this needs an explicit entry ABI rather than an undocumented register assumption.
3. Consider a separate legacy-Z80 MOS target only if 16-bit `/mos` command support is a project goal. It should not be folded into the existing ADL target.
