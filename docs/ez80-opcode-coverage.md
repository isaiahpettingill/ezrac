# eZ80 Assembler Opcode Coverage

EZRA's internal assembler started as a test/build subset for emitted compiler output. The goal is full Zilog UM0077 mnemonic coverage for Agon/eZ80 work.

Current expanded coverage includes:

- Base 8-bit register loads, immediate loads, `(hl)` loads/stores, and direct loads/stores used by EZRA output.
- Base ALU operations for registers, immediates, `(hl)`, and `(ix/iy+d)`.
- Base control flow: `jp`, `jr`, `djnz`, `call`, `ret`, conditional `jp`/`call`/`ret`, `rst`, and `rst.lis`.
- Common 16-bit register operations: `inc`, `dec`, `add hl`, `adc hl`, `sbc hl`, IX/IY arithmetic, stack push/pop, `ld sp,*`, `ex`, and indirect `jp`.
- CB-prefixed register, `(hl)`, and `(ix/iy+d)` rotate/shift/bit/set/res operations currently needed by compatibility work.
- ED-prefixed block operations, `mlt`, interrupt mode, interrupt returns, special `i`/`r` register loads, `rld`, and `rrd`.
- Standard `in`/`out` forms plus eZ80 `in0`/`out0` forms used by the runtime and Agon SDK.
- Standalone CLI assembly through `ezra assemble --base <addr> --output <file.bin> <file.asm>`.

Remaining UM0077 work:

- IXH/IYH/IXL/IYL aliases and any remaining IX/IY syntax variants not covered by register, direct, indexed, stack, and CB forms above.
- Full eZ80 suffix/prefix mode variants including `.sis`, `.lil`, `.sil`, and `.lis` where applicable.
- Any remaining eZ80-specific I/O forms beyond standard `in`/`out` and current `in0`/`out0` support.
- Exhaustive operand aliases and syntax variants used by third-party assemblers.
- A generated opcode table test suite cross-checking all accepted mnemonics against UM0077 encodings.
