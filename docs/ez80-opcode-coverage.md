# eZ80 Assembler Opcode Coverage

EZRA's internal assembler started as a test/build subset for emitted compiler output. The goal is full Zilog UM0077 mnemonic coverage for Agon/eZ80 work.

Current expanded coverage includes:

- Base 8-bit register loads, immediate loads, `(hl)` loads/stores, and direct loads/stores used by EZRA output.
- Base ALU operations for registers, immediates, and `(hl)`.
- Base control flow: `jp`, `jr`, `djnz`, `call`, `ret`, conditional `jp`/`call`/`ret`, `rst`, and `rst.lis`.
- Common 16-bit register operations: `inc`, `dec`, `add hl`, `adc hl`, `sbc hl`, stack push/pop, `ld sp,*`, `ex`, and indirect `jp`.
- CB-prefixed register and `(hl)` rotate/shift/bit/set/res operations currently needed by compatibility work.
- ED-prefixed block operations, `mlt`, interrupt mode, interrupt returns, special `i`/`r` register loads, `rld`, and `rrd`.
- eZ80 `in0`/`out0` forms used by the runtime and Agon SDK.

Remaining UM0077 work:

- Complete IX/IY indexed forms beyond the current `ld a,(ix+d)` and `ld (ix+d),a` subset.
- Full eZ80 suffix/prefix mode variants including `.sis`, `.lil`, `.sil`, and `.lis` where applicable.
- Full I/O forms beyond `in0`/`out0`.
- Exhaustive operand aliases and syntax variants used by third-party assemblers.
- A generated opcode table test suite cross-checking all accepted mnemonics against UM0077 encodings.
