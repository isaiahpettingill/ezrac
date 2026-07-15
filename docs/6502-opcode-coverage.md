# MOS 6502 Assembler Opcode Coverage

The assembler supports four 6502-family variants via the `Mos6502Variant` enum (`Nmos6502`, `Cmos65C02`, `Wdc65C816`, `Ricoh2A03`). All variants share a common NMOS 6502 baseline with per-variant additions.

## NMOS 6502 (all variants)

All documented NMOS 6502 instructions are supported across every variant, subject to the Ricoh 2A03 differences noted below.

### Implied (1 byte)

`brk` `php` `clc` `plp` `sec` `rti` `pha` `cli` `rts` `pla` `sei` `dey` `txa` `tya` `txs` `tay` `tax` `clv` `tsx` `iny` `dex` `cld` `inx` `nop` `sed`

### Accumulator (1 byte)

`asl A` `rol A` `lsr A` `ror A`

### Immediate (2 bytes: opcode + byte)

`ora #imm` `and #imm` `eor #imm` `adc #imm` `lda #imm` `cmp #imm` `sbc #imm` (`$E9`) `ldy #imm` `ldx #imm` `cpy #imm` `cpx #imm`

### Zero Page (2 bytes: opcode + addr)

`ora zp` `and zp` `eor zp` `adc zp` `sta zp` `lda zp` `cmp zp` `sbc zp` `asl zp` `rol zp` `lsr zp` `ror zp` `bit zp` `sty zp` `stx zp` `ldy zp` `ldx zp` `cpy zp` `cpx zp` `dec zp` `inc zp`

### Zero Page, X (2 bytes: opcode + addr)

`ora zp,x` `and zp,x` `eor zp,x` `adc zp,x` `sta zp,x` `lda zp,x` `cmp zp,x` `sbc zp,x` `asl zp,x` `rol zp,x` `lsr zp,x` `ror zp,x` `sty zp,x` `ldy zp,x` `dec zp,x` `inc zp,x`

### Zero Page, Y (2 bytes: opcode + addr)

`stx zp,y` `ldx zp,y`

### Absolute (3 bytes: opcode + addr lo/hi)

`jsr abs` `jmp abs` `ora abs` `and abs` `eor abs` `adc abs` `sta abs` `lda abs` `cmp abs` `sbc abs` `asl abs` `rol abs` `lsr abs` `ror abs` `bit abs` `sty abs` `stx abs` `ldy abs` `ldx abs` `cpy abs` `cpx abs` `dec abs` `inc abs`

### Absolute, X (3 bytes: opcode + addr lo/hi)

`ora abs,x` `and abs,x` `eor abs,x` `adc abs,x` `sta abs,x` `lda abs,x` `cmp abs,x` `sbc abs,x` `asl abs,x` `rol abs,x` `lsr abs,x` `ror abs,x` `ldy abs,x` `dec abs,x` `inc abs,x`

### Absolute, Y (3 bytes: opcode + addr lo/hi)

`ora abs,y` `and abs,y` `eor abs,y` `adc abs,y` `sta abs,y` `lda abs,y` `cmp abs,y` `sbc abs,y` `ldx abs,y`

### Indirect (3 bytes: opcode + addr lo/hi)

`jmp (ind)`

### Indexed Indirect — (zp,X) (2 bytes: opcode + zp addr)

`ora (zp,x)` `and (zp,x)` `eor (zp,x)` `adc (zp,x)` `sta (zp,x)` `lda (zp,x)` `cmp (zp,x)` `sbc (zp,x)`

### Indirect Indexed — (zp),Y (2 bytes: opcode + zp addr)

`ora (zp),y` `and (zp),y` `eor (zp),y` `adc (zp),y` `sta (zp),y` `lda (zp),y` `cmp (zp),y` `sbc (zp),y`

### Relative branches (2 bytes: opcode + signed offset)

`bpl rel` `bmi rel` `bvc rel` `bvs rel` `bcc rel` `bcs rel` `bne rel` `beq rel`

## CMOS 65C02 additions (available on `Cmos65C02` and `Wdc65C816`)

All above NMOS opcodes are inherited. These are added:

### New implied (1 byte)

`phx` `phy` `plx` `ply` `stp` `wai`

### New accumulator (1 byte)

`inc A` `dec A`

### New immediate (2 bytes)

`bit #imm` (`$89`)

### New relative branch (2 bytes)

`bra rel` (`$80`)

### New zero-page indirect — (zp) (2 bytes)

`ora (zp)` `and (zp)` `eor (zp)` `adc (zp)` `sta (zp)` `lda (zp)` `cmp (zp)` `sbc (zp)`

### New indexed indirect — JMP (abs,X) (3 bytes)

`jmp (abs,x)` (`$7C`)

### STZ (zero page, absolute, ZP/X, abs/X)

`stz zp` `stz abs` `stz zp,x` `stz abs,x`

### TRB / TSB (zero page, absolute)

`trb zp` `trb abs` `tsb zp` `tsb abs`

### RMB / SMB (zero page, 1 byte operand)

`rmb0 zp`–`rmb7 zp` `smb0 zp`–`smb7 zp`

### BBR / BBS (zero page + relative, 3 bytes)

`bbr0 zp,rel`–`bbr7 zp,rel` `bbs0 zp,rel`–`bbs7 zp,rel`

## WDC 65C816 additions (available on `Wdc65C816` only)

All above NMOS and 65C02 opcodes are inherited. These are added:

### New implied (1 byte)

`xba` `xce` `rtl` `phb` `phd` `phk` `plb` `pld` `tcs` `tsc` `tcd` `tdc` `txy` `tyx`

### New immediate (2 bytes)

`cop #imm` `rep #imm` `sep #imm` `wdm #imm`

### New relative long — BRL (3 bytes: opcode + 16-bit signed offset)

`brl rel`

### PER — PC-relative long (3 bytes)

`per rel`

### Block move (3 bytes: opcode + src bank, dst bank)

`mvp src,dst` `mvn src,dst`

### Push effective address

`pea abs` (3 bytes) `pei (zp)` (2 bytes) `per rel` (3 bytes)

### JSR/JMP indexed indirect (with 16-bit base)

`jsr (abs,x)` `jmp (abs,x)`

### Absolute long (4 bytes: opcode + addr lo/mid/hi)

`jmp far` `jsr far` `lda far` `sta far` `adc far` `sbc far` `and far` `ora far` `eor far` `cmp far`

### Indirect long — JMP [addr] (3 bytes: opcode + addr lo/hi, 24-bit target)

`jmp [ind]`

## Ricoh 2A03 differences (available on `Ricoh2A03` only)

All NMOS 6502 opcodes are supported, with one encoding difference:

- **SBC immediate** uses `$EB` instead of the standard NMOS `$E9`
- All other SBC modes (zp, abs, indexed, indirect) use standard NMOS encodings — only the immediate form differs

Ricoh 2A03 rejects 65C02 and 65C816 opcodes.

## Addressing mode selection

| Syntax | NMOS | Cmos65C02 | Wdc65C816 | Ricoh2A03 |
|---|---|---|---|---|
| `$val` (val ≤ 0xFF) | ZeroPage | ZeroPage | ZeroPage | ZeroPage |
| `$val` (val > 0xFF) | Absolute | Absolute | Absolute | Absolute |
| `$val,x` (val ≤ 0xFF) | ZeroPageX | ZeroPageX | ZeroPageX | ZeroPageX |
| `$val,x` (val > 0xFF) | AbsoluteX | AbsoluteX | AbsoluteX | AbsoluteX |
| `$val,y` (val ≤ 0xFF) | ZeroPageY | ZeroPageY | ZeroPageY | ZeroPageY |
| `$val,y` (val > 0xFF) | AbsoluteY | AbsoluteY | AbsoluteY | AbsoluteY |
| `($val,x)` | IndexedIndirect | IndexedIndirect | IndexedIndirect | IndexedIndirect |
| `($val),y` | IndirectIndexed | IndirectIndexed | IndirectIndexed | IndirectIndexed |
| `($val)` for ADC/AND/CMP/EOR/LDA/ORA/SBC/STA | — | ZeroPageIndirect | ZeroPageIndirect | — |
| `($val)` for JMP | Indirect | Indirect | Indirect | Indirect |
| `($val,x)` for JMP/JSR with val > 0xFF | — | IndexedIndirectX | IndexedIndirectX | — |
| `!$val` prefix | — | — | AbsoluteLong | — |
| `[$val]` | — | — | IndirectLong | — |
| `#val` | Immediate | Immediate | Immediate | Immediate |

## Variant selection

In EZRA source code, specify the target assembler CPU in `.ezra.toml`:

```toml
[build]
assembler_cpu = "6502"        # NMOS 6502 (default)
assembler_cpu = "65c02"       # CMOS 65C02
assembler_cpu = "65c816"      # WDC 65C816
assembler_cpu = "2a03"        # Ricoh 2A03 (NES)
```

From the CLI assemble command:

```
ezra assemble --cpu 65c02 --base 0x8000 --output out.bin in.asm
```

## Roadmap

- Existing coverage is sufficient for Commodore 64 and NES test builds.
- 65C02 and 65C816 support is assembler-only; emitter/codegen paths still target NMOS 6502.
- Exhaustive WDC datasheet coverage: cross-checked against WDC 65C02 (Table 5-2), WDC 65C816 datasheets, and oxyron opcode matrices.
- Known gaps: 65C816 STP/WAI (inherited from 65C02, present), some MVN/MVP addressing variants, and 65C02 `(zp)` indirect for JMP (uses standard `Indirect` mode like NMOS). All other documented opcodes for these variants are covered.
- Priority is maintaining correctness for emitted compiler output. New opcodes are added when EZRA source or SDK examples require them.
