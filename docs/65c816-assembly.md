# 65C816 / 65816 assembly

EZRA assembles the WDC 65C816 instruction set with `--assembler-cpu 65c816`.
`65816` and `wdc65c816` are accepted aliases. The canonical target CPU name is
`65c816`.

`generic-65c816-bare` and `generic-65816-bare` produce raw 24-bit-addressable
binaries. Their default layout places code in bank `$00` from `$00:8000` to
`$00:FFFF`; the entry point is `$00:8000`. This keeps the initial source ABI's
near `JSR`/`RTS` calls in one program bank. Direct page is `$00:0000-$00:00FF`
and the native stack area is `$00:0100-$00:1FFF`. A raw binary has no header or
reset vector. The generated startup enters native mode, initializes the stack
to `$1FFF`, and uses an 8-bit A/X/Y byte ABI. A custom loader must place the
binary at the entry address and transfer control there.

## Register-width state

The 65C816 starts in emulation mode, so the assembler starts with 8-bit A, X,
and Y. Immediate forms of accumulator instructions (`ADC`, `AND`, `BIT`, `CMP`,
`EOR`, `LDA`, `ORA`, and `SBC`) use the tracked A width. `CPX`, `CPY`, `LDX`,
and `LDY` use the tracked X/Y width.

`REP #mask` and `SEP #mask` update the state when their masks contain M (`$20`)
or X (`$10`). Use these directives when the width is established outside the
current source file or after code whose processor state cannot be inferred:

```asm
.a16
.i16
lda #$1234
ldx #$5678
sep #$30
lda #$12
ldx #$34
```

The directives emit no bytes. `.a8` and `.a16` select the accumulator width;
`.i8` and `.i16` select the index-register width. They are valid only with the
65C816 assembler CPU.

## Native-only addressing syntax

- `!$12:3456` or `!$123456`: absolute long
- `!$12:3456,x`: absolute long indexed by X
- `[$12]`, `[$12],y`: direct-page indirect long
- `$12,s`, `($12,s),y`: stack-relative and stack-relative indirect indexed by Y
- `jml !$12:3456`, `jsl !$12:3456`: long jump and long subroutine call

The existing `jmp !$12:3456` and `jsr !$12:3456` spellings remain accepted.

## SNES note

The SNES Ricoh 5A22 is not a generic 65C816 target. It shares 65C816 opcode
encodings, but SNES startup, vectors, banks, timing, DMA, PPU/APU registers,
and controller I/O are platform behavior. A later `snes-5a22` target will use
the shared 65C816 encoder only where its instruction behavior matches and will
provide its own layout, ROM packaging, and SDK.
