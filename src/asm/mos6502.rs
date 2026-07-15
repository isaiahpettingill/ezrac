use std::collections::HashMap;

use crate::diagnostic::Diagnostic;
use crate::vm::parse_number;

/// MOS 6502 CPU variant for variant-aware assembly.
///
/// Defaults to `Nmos6502` in the legacy API. Use `encode_instruction_for_variant`
/// or `instruction_len_for_variant` to select a specific variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mos6502Variant {
    /// Original NMOS 6502 (1975). 16-bit address space.
    /// Used by Commodore 64, Apple II, Atari 2600, etc.
    Nmos6502,
    /// CMOS 65C02 (WDC). Adds BRA, (zp) indirect, JMP (abs,X),
    /// STZ, TRB/TSB, RMB/SMB, BBR/BBS, INC/DEC A, and BIT #imm.
    /// 16-bit address space. Superset of NMOS.
    Cmos65C02,
    /// WDC 65C816 (65816). 24-bit address space. Adds long addressing,
    /// block moves, new transfers/registers, and expanded stack ops.
    /// Superset of 65C02 and NMOS.
    Wdc65C816,
    /// Ricoh 2A03 (NES CPU). NMOS-based with one encoding difference:
    /// SBC immediate uses `$EB` instead of `$E9`. Rejects 65C02/65C816 opcodes.
    Ricoh2A03,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Implied,
    Accumulator,
    Immediate,
    ZeroPage,
    ZeroPageX,
    ZeroPageY,
    Absolute,
    AbsoluteX,
    AbsoluteY,
    Indirect,
    ZeroPageIndirect,
    IndexedIndirect,
    IndirectIndexed,
    Relative,
    AbsoluteLong,
    IndirectLong,
    IndexedIndirectX,
    RelativeLong,
}

/// Returns the assembled length of a single MOS 6502 instruction in bytes.
///
/// Uses the default NMOS 6502 variant. For variant-aware length,
/// use [`instruction_len_for_variant`].
pub fn instruction_len(text: &str) -> Result<usize, Diagnostic> {
    Ok(encode(text, &HashMap::new(), 0, false, Mos6502Variant::Nmos6502)?.len())
}

/// Returns the assembled length of a single instruction for a specific
/// 6502-family variant (NMOS, 65C02, 65C816, or 2A03).
pub fn instruction_len_for_variant(
    text: &str,
    variant: Mos6502Variant,
) -> Result<usize, Diagnostic> {
    Ok(encode(text, &HashMap::new(), 0, false, variant)?.len())
}

/// Encodes a single MOS 6502 instruction string into bytes.
///
/// Uses the default NMOS 6502 variant. For variant-aware encoding,
/// use [`encode_instruction_for_variant`].
///
/// `labels` provides known symbol values; `pc` is the current program counter;
/// `resolve` controls whether forward references are resolved or left as defaults.
pub fn encode_instruction(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    encode(text, labels, pc, resolve, Mos6502Variant::Nmos6502)
}

/// Encodes a single instruction for a specific 6502-family variant
/// (NMOS, 65C02, 65C816, or 2A03).
///
/// See [`encode_instruction`] for parameter documentation.
pub fn encode_instruction_for_variant(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
    variant: Mos6502Variant,
) -> Result<Vec<u8>, Diagnostic> {
    encode(text, labels, pc, resolve, variant)
}

fn encode(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
    variant: Mos6502Variant,
) -> Result<Vec<u8>, Diagnostic> {
    let normalized = text.trim().to_ascii_lowercase();
    let (mnemonic, operand) = normalized
        .split_once(char::is_whitespace)
        .map_or((normalized.as_str(), ""), |(op, rest)| (op, rest.trim()));

    // BBR/BBS: opcode, zp_addr, relative_offset (3 bytes total)
    if (variant == Mos6502Variant::Cmos65C02 || variant == Mos6502Variant::Wdc65C816)
        && is_bit_branch(mnemonic)
    {
        let parts: Vec<&str> = operand.splitn(2, ',').collect();
        if parts.len() != 2 {
            return Err(Diagnostic::new(
                "expected `zp_addr,label` for BBR/BBS instruction".to_string(),
            ));
        }
        let zp = value_or(parts[0], labels, pc, resolve, 0)? as u8;
        let target = value_or(parts[1], labels, pc, resolve, 0)?;
        if let Some(bytes) = bbr_bbs_opcode(mnemonic, u32::from(zp), target, pc) {
            return Ok(bytes);
        }
        return Err(Diagnostic::new(format!(
            "assembler does not support {} instruction `{text}`",
            variant.as_str()
        )));
    }

    let (mode, value) = parse_operand(mnemonic, operand, labels, pc, resolve, variant)?;
    let opcode = opcode(mnemonic, mode, variant).ok_or_else(|| {
        Diagnostic::new(format!(
            "assembler does not support {} instruction `{text}`",
            variant.as_str()
        ))
    })?;
    let mut out = vec![opcode];
    match mode {
        Mode::Implied | Mode::Accumulator => {}
        Mode::Immediate
        | Mode::ZeroPage
        | Mode::ZeroPageX
        | Mode::ZeroPageY
        | Mode::IndexedIndirect
        | Mode::IndirectIndexed => out.push(u8_value(operand, value)?),
        Mode::Relative => out.push(relative_offset_6502(pc, value)?),
        Mode::RelativeLong => {
            let offset = i64::from(value as i16) - (pc.wrapping_add(3) as i64);
            let low = (offset as i16).to_le_bytes();
            out.push(low[0]);
            out.push(low[1]);
        }
        Mode::ZeroPageIndirect => out.push(u8_value(operand, value)?),
        Mode::Absolute
        | Mode::AbsoluteX
        | Mode::AbsoluteY
        | Mode::Indirect
        | Mode::IndexedIndirectX => push16(&mut out, value)?,
        Mode::AbsoluteLong => push24(&mut out, value)?,
        Mode::IndirectLong => push16(&mut out, value)?,
    }
    Ok(out)
}

fn parse_operand(
    mnemonic: &str,
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
    variant: Mos6502Variant,
) -> Result<(Mode, u32), Diagnostic> {
    if operand.is_empty() {
        return Ok((Mode::Implied, 0));
    }
    if operand == "a" {
        return Ok((Mode::Accumulator, 0));
    }
    if let Some(expr) = operand.strip_prefix('#') {
        return Ok((Mode::Immediate, value_or(expr, labels, pc, resolve, 0)?));
    }
    if is_branch_long(mnemonic) {
        return Ok((
            Mode::RelativeLong,
            value_or(operand, labels, pc, resolve, 0)?,
        ));
    }
    if is_branch(mnemonic) {
        return Ok((Mode::Relative, value_or(operand, labels, pc, resolve, 0)?));
    }
    if mnemonic == "per" {
        return Ok((
            Mode::RelativeLong,
            value_or(operand, labels, pc, resolve, 0)?,
        ));
    }
    if mnemonic == "mvp" || mnemonic == "mvn" {
        let parts: Vec<&str> = operand.split(',').collect();
        if parts.len() == 2 {
            let src = value_or(parts[0], labels, pc, resolve, 0)? as u8;
            let dst = value_or(parts[1], labels, pc, resolve, 0)? as u8;
            return Ok((Mode::Absolute, u32::from(src) << 8 | u32::from(dst)));
        }
    }

    if mnemonic == "pei" {
        if let Some(inner) = operand.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            let v = value_or(inner, labels, pc, resolve, 0)?;
            return Ok((Mode::IndexedIndirect, v));
        }
    }

    if let Some(inner) = operand
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(",x)"))
    {
        let v = value_or(inner, labels, pc, resolve, 0)?;
        if variant != Mos6502Variant::Nmos6502
            && variant != Mos6502Variant::Ricoh2A03
            && operand_is_numeric(inner)
            && v > 0xff
        {
            return Ok((Mode::IndexedIndirectX, v));
        }
        return Ok((Mode::IndexedIndirect, v));
    }
    if let Some(inner) = operand
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix("),y"))
    {
        return Ok((
            Mode::IndirectIndexed,
            value_or(inner, labels, pc, resolve, 0)?,
        ));
    }
    if let Some(inner) = operand.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if variant == Mos6502Variant::Wdc65C816 {
            return Ok((
                Mode::IndirectLong,
                value_or(inner, labels, pc, resolve, 0x100)?,
            ));
        }
        return Err(Diagnostic::new(format!(
            "assembler does not support {} instruction `{mnemonic} [{inner}]`",
            variant.as_str()
        )));
    }
    if let Some(inner) = operand.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        let v = value_or(inner, labels, pc, resolve, 0x100)?;
        // On 65C02/65C816, ($12) for ALU ops is (zp) indirect, not JMP indirect
        if variant != Mos6502Variant::Nmos6502
            && variant != Mos6502Variant::Ricoh2A03
            && matches!(
                mnemonic,
                "adc" | "and" | "cmp" | "eor" | "lda" | "ora" | "sbc" | "sta"
            )
        {
            return Ok((
                Mode::ZeroPageIndirect,
                value_or(inner, labels, pc, resolve, 0)?,
            ));
        }
        return Ok((Mode::Indirect, v));
    }
    if let Some(expr) = operand.strip_suffix(",x") {
        let v = value_or(expr, labels, pc, resolve, 0x100)?;
        return Ok((
            if operand_is_numeric(expr) && v <= 0xff {
                Mode::ZeroPageX
            } else {
                Mode::AbsoluteX
            },
            v,
        ));
    }
    if let Some(expr) = operand.strip_suffix(",y") {
        let v = value_or(expr, labels, pc, resolve, 0x100)?;
        return Ok((
            if operand_is_numeric(expr) && v <= 0xff {
                Mode::ZeroPageY
            } else {
                Mode::AbsoluteY
            },
            v,
        ));
    }
    if let Some(expr) = operand.strip_suffix(",s") {
        if variant == Mos6502Variant::Wdc65C816 {
            let v = value_or(expr, labels, pc, resolve, 0x100)?;
            return Ok((Mode::ZeroPage, v));
        }
    }
    let v = value_or(operand, labels, pc, resolve, 0x100)?;
    if operand.starts_with('>') || operand.starts_with('^') {
        let prefix = &operand[..1];
        let inner = operand[1..].trim();
        let v = value_or(inner, labels, pc, resolve, 0x100)?;
        match prefix {
            ">" => {
                if variant == Mos6502Variant::Wdc65C816 {
                    return Ok((Mode::Absolute, v));
                }
            }
            "^" => {
                if variant == Mos6502Variant::Wdc65C816 {
                    let bank = (v >> 16) as u8;
                    return Ok((Mode::Immediate, u32::from(bank)));
                }
            }
            _ => {}
        }
    }
    if operand_is_numeric(operand)
        && v <= 0x00ff_ffff
        && variant == Mos6502Variant::Wdc65C816
        && matches!(
            mnemonic,
            "jmp" | "jsr" | "lda" | "sta" | "adc" | "sbc" | "and" | "ora" | "eor" | "cmp"
        )
    {
        let explicit_long = operand.contains(':') || operand.starts_with('!');
        if v > 0xffff || explicit_long {
            let inner = if explicit_long {
                operand.trim_start_matches('!')
            } else {
                operand
            };
            let mode = match mnemonic {
                "jmp" if explicit_long => Mode::AbsoluteLong,
                "jsr" if explicit_long => Mode::AbsoluteLong,
                _ => Mode::AbsoluteLong,
            };
            let actual_v = if explicit_long {
                value_or(inner, labels, pc, resolve, 0x100)?
            } else {
                v
            };
            return Ok((mode, actual_v));
        }
    }
    Ok((
        if operand_is_numeric(operand) && v <= 0xff {
            Mode::ZeroPage
        } else {
            Mode::Absolute
        },
        v,
    ))
}

fn operand_is_numeric(expr: &str) -> bool {
    let expr = expr.trim();
    expr.starts_with('>')
        || expr.starts_with('^')
        || expr.starts_with('!')
        || expr == "$"
        || expr.starts_with('$')
        || parse_number(expr).is_ok()
}

fn value_or(
    expr: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
    unresolved: u32,
) -> Result<u32, Diagnostic> {
    let expr = expr.trim();
    if expr == "$" {
        return Ok(pc);
    }
    if let Some(hex) = expr.strip_prefix('$') {
        return u32::from_str_radix(hex, 16)
            .map_err(|_| Diagnostic::new(format!("invalid numeric operand `{expr}`")));
    }
    if let Some(v) = labels.get(expr).copied().or_else(|| {
        labels
            .iter()
            .find_map(|(n, v)| n.eq_ignore_ascii_case(expr).then_some(*v))
    }) {
        return Ok(v);
    }
    match parse_number(expr) {
        Ok(value) => Ok(value),
        Err(_) if !resolve => Ok(unresolved),
        Err(error) => Err(error),
    }
}

fn u8_value(operand: &str, value: u32) -> Result<u8, Diagnostic> {
    u8::try_from(value)
        .map_err(|_| Diagnostic::new(format!("6502 operand `{operand}` is outside u8 range")))
}

fn relative_offset_6502(pc: u32, target: u32) -> Result<u8, Diagnostic> {
    let pc = u16::try_from(pc).map_err(|_| {
        Diagnostic::new(format!(
            "6502 program counter 0x{pc:X} is outside 16-bit range"
        ))
    })?;
    let target = u16::try_from(target).map_err(|_| {
        Diagnostic::new(format!(
            "6502 branch target 0x{target:X} is outside 16-bit range"
        ))
    })?;
    let displacement = target.wrapping_sub(pc.wrapping_add(2));
    if !(displacement <= 0x007f || displacement >= 0xff80) {
        return Err(Diagnostic::new(format!(
            "6502 branch target 0x{target:04X} is out of range from 0x{pc:04X}"
        )));
    }
    Ok(displacement as u8)
}

fn push16(out: &mut Vec<u8>, value: u32) -> Result<(), Diagnostic> {
    let v = u16::try_from(value).map_err(|_| {
        Diagnostic::new(format!("6502 address 0x{value:X} is outside 16-bit range"))
    })?;
    out.extend(v.to_le_bytes());
    Ok(())
}

fn push24(out: &mut Vec<u8>, value: u32) -> Result<(), Diagnostic> {
    if value > 0xffffff {
        return Err(Diagnostic::new(format!(
            "6502 address 0x{value:X} is outside 24-bit range"
        )));
    }
    out.extend(value.to_le_bytes()[..3].iter());
    Ok(())
}

fn is_branch(m: &str) -> bool {
    matches!(
        m,
        "bcc" | "bcs" | "beq" | "bmi" | "bne" | "bpl" | "bvc" | "bvs" | "bra"
    )
}

fn is_branch_long(m: &str) -> bool {
    matches!(m, "brl")
}

impl Mos6502Variant {
    /// Returns the short display name for this variant (e.g. `"65C02"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Mos6502Variant::Nmos6502 => "6502",
            Mos6502Variant::Cmos65C02 => "65C02",
            Mos6502Variant::Wdc65C816 => "65C816",
            Mos6502Variant::Ricoh2A03 => "2A03",
        }
    }
}

fn opcode(m: &str, mode: Mode, variant: Mos6502Variant) -> Option<u8> {
    match (m, mode, variant) {
        ("brk", Mode::Implied, _) => Some(0x00),
        ("php", Mode::Implied, _) => Some(0x08),
        ("clc", Mode::Implied, _) => Some(0x18),
        ("plp", Mode::Implied, _) => Some(0x28),
        ("sec", Mode::Implied, _) => Some(0x38),
        ("rti", Mode::Implied, _) => Some(0x40),
        ("pha", Mode::Implied, _) => Some(0x48),
        ("cli", Mode::Implied, _) => Some(0x58),
        ("rts", Mode::Implied, _) => Some(0x60),
        ("pla", Mode::Implied, _) => Some(0x68),
        ("sei", Mode::Implied, _) => Some(0x78),
        ("dey", Mode::Implied, _) => Some(0x88),
        ("txa", Mode::Implied, _) => Some(0x8A),
        ("tya", Mode::Implied, _) => Some(0x98),
        ("txs", Mode::Implied, _) => Some(0x9A),
        ("tay", Mode::Implied, _) => Some(0xA8),
        ("tax", Mode::Implied, _) => Some(0xAA),
        ("clv", Mode::Implied, _) => Some(0xB8),
        ("tsx", Mode::Implied, _) => Some(0xBA),
        ("iny", Mode::Implied, _) => Some(0xC8),
        ("dex", Mode::Implied, _) => Some(0xCA),
        ("cld", Mode::Implied, _) => Some(0xD8),
        ("inx", Mode::Implied, _) => Some(0xE8),
        ("nop", Mode::Implied, _) => Some(0xEA),
        ("sed", Mode::Implied, _) => Some(0xF8),
        ("asl", Mode::Accumulator, _) => Some(0x0A),
        ("rol", Mode::Accumulator, _) => Some(0x2A),
        ("lsr", Mode::Accumulator, _) => Some(0x4A),
        ("ror", Mode::Accumulator, _) => Some(0x6A),
        ("jsr", Mode::Absolute, _) => Some(0x20),
        ("jmp", Mode::Absolute, _) => Some(0x4C),
        ("jmp", Mode::Indirect, _) => Some(0x6C),
        ("bpl", Mode::Relative, _) => Some(0x10),
        ("bmi", Mode::Relative, _) => Some(0x30),
        ("bvc", Mode::Relative, _) => Some(0x50),
        ("bvs", Mode::Relative, _) => Some(0x70),
        ("bcc", Mode::Relative, _) => Some(0x90),
        ("bcs", Mode::Relative, _) => Some(0xB0),
        ("bne", Mode::Relative, _) => Some(0xD0),
        ("beq", Mode::Relative, _) => Some(0xF0),
        ("ora", Mode::IndexedIndirect, _) => Some(0x01),
        ("and", Mode::IndexedIndirect, _) => Some(0x21),
        ("eor", Mode::IndexedIndirect, _) => Some(0x41),
        ("adc", Mode::IndexedIndirect, _) => Some(0x61),
        ("sta", Mode::IndexedIndirect, _) => Some(0x81),
        ("lda", Mode::IndexedIndirect, _) => Some(0xA1),
        ("cmp", Mode::IndexedIndirect, _) => Some(0xC1),
        ("sbc", Mode::IndexedIndirect, _) => Some(0xE1),
        ("ora", Mode::IndirectIndexed, _) => Some(0x11),
        ("and", Mode::IndirectIndexed, _) => Some(0x31),
        ("eor", Mode::IndirectIndexed, _) => Some(0x51),
        ("adc", Mode::IndirectIndexed, _) => Some(0x71),
        ("sta", Mode::IndirectIndexed, _) => Some(0x91),
        ("lda", Mode::IndirectIndexed, _) => Some(0xB1),
        ("cmp", Mode::IndirectIndexed, _) => Some(0xD1),
        ("sbc", Mode::IndirectIndexed, _) => Some(0xF1),

        // 65C02
        ("bra", Mode::Relative, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0x80)
        }
        ("phx", Mode::Implied, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => Some(0xDA),
        ("phy", Mode::Implied, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => Some(0x5A),
        ("plx", Mode::Implied, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => Some(0xFA),
        ("ply", Mode::Implied, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => Some(0x7A),
        ("stp", Mode::Implied, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => Some(0xDB),
        ("wai", Mode::Implied, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => Some(0xCB),
        ("inc", Mode::Accumulator, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0x1A)
        }
        ("dec", Mode::Accumulator, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0x3A)
        }
        // 65C02 BIT immediate and JMP (abs,X)
        ("bit", Mode::Immediate, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0x89)
        }
        ("jmp", Mode::IndexedIndirectX, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0x7C)
        }

        // 65C02 (zp) indirect for ALU/load-store
        ("ora", Mode::ZeroPageIndirect, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0x12)
        }
        ("and", Mode::ZeroPageIndirect, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0x32)
        }
        ("eor", Mode::ZeroPageIndirect, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0x52)
        }
        ("adc", Mode::ZeroPageIndirect, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0x72)
        }
        ("sta", Mode::ZeroPageIndirect, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0x92)
        }
        ("lda", Mode::ZeroPageIndirect, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0xB2)
        }
        ("cmp", Mode::ZeroPageIndirect, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0xD2)
        }
        ("sbc", Mode::ZeroPageIndirect, Mos6502Variant::Cmos65C02 | Mos6502Variant::Wdc65C816) => {
            Some(0xF2)
        }

        // 65C816 XBA, XCE, COP, BRL, RTL
        ("xba", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0xEB),
        ("xce", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0xFB),
        ("cop", Mode::Immediate, Mos6502Variant::Wdc65C816) => Some(0x02),
        ("brl", Mode::RelativeLong, Mos6502Variant::Wdc65C816) => Some(0x82),
        ("rtl", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0x6B),

        // 65C816 REP, SEP
        ("rep", Mode::Immediate, Mos6502Variant::Wdc65C816) => Some(0xC2),
        ("sep", Mode::Immediate, Mos6502Variant::Wdc65C816) => Some(0xE2),

        // 65C816 block move
        ("mvp", Mode::Absolute, Mos6502Variant::Wdc65C816) => Some(0x44),
        ("mvn", Mode::Absolute, Mos6502Variant::Wdc65C816) => Some(0x54),

        // 65C816 push effective address
        ("pea", Mode::Absolute, Mos6502Variant::Wdc65C816) => Some(0xF4),
        ("pei", Mode::IndexedIndirect, Mos6502Variant::Wdc65C816) => Some(0xD4),
        ("per", Mode::RelativeLong, Mos6502Variant::Wdc65C816) => Some(0x62),

        // 65C816 JSR indexed indirect and long variants
        ("jsr", Mode::IndexedIndirectX, Mos6502Variant::Wdc65C816) => Some(0xFC),
        ("jmp", Mode::AbsoluteLong, Mos6502Variant::Wdc65C816) => Some(0x5C),
        ("jsr", Mode::AbsoluteLong, Mos6502Variant::Wdc65C816) => Some(0x22),
        ("jmp", Mode::IndirectLong, Mos6502Variant::Wdc65C816) => Some(0xDC),

        // 65C816 long addressing for ALU/load/store
        ("lda", Mode::AbsoluteLong, Mos6502Variant::Wdc65C816) => Some(0xAF),
        ("sta", Mode::AbsoluteLong, Mos6502Variant::Wdc65C816) => Some(0x8F),
        ("adc", Mode::AbsoluteLong, Mos6502Variant::Wdc65C816) => Some(0x6F),
        ("sbc", Mode::AbsoluteLong, Mos6502Variant::Wdc65C816) => Some(0xEF),
        ("and", Mode::AbsoluteLong, Mos6502Variant::Wdc65C816) => Some(0x2F),
        ("ora", Mode::AbsoluteLong, Mos6502Variant::Wdc65C816) => Some(0x0F),
        ("eor", Mode::AbsoluteLong, Mos6502Variant::Wdc65C816) => Some(0x4F),
        ("cmp", Mode::AbsoluteLong, Mos6502Variant::Wdc65C816) => Some(0xCF),

        // 65C816 push/pull bank/DP registers
        ("phb", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0x8B),
        ("phd", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0x0B),
        ("phk", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0x4B),
        ("plb", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0xAB),
        ("pld", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0x2B),

        // 65C816 transfer instructions
        ("tcs", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0x1B),
        ("tsc", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0x3B),
        ("tcd", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0x5B),
        ("tdc", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0x7B),
        ("txy", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0x9B),
        ("tyx", Mode::Implied, Mos6502Variant::Wdc65C816) => Some(0xBB),

        // 65C816 WDM
        ("wdm", Mode::Immediate, Mos6502Variant::Wdc65C816) => Some(0x42),

        // Ricoh 2A03: standard NMOS but SBC immediate uses $EB not $E9
        ("sbc", Mode::Immediate, Mos6502Variant::Ricoh2A03) => Some(0xEB),

        _ => return opcode_group(m, mode, variant),
    }
}

fn opcode_group(m: &str, mode: Mode, variant: Mos6502Variant) -> Option<u8> {
    let rows: &[(&str, &[(Mode, u8)])] = &[
        (
            "ora",
            &[
                (Mode::ZeroPage, 0x05),
                (Mode::Immediate, 0x09),
                (Mode::Absolute, 0x0D),
                (Mode::ZeroPageX, 0x15),
                (Mode::AbsoluteX, 0x1D),
                (Mode::AbsoluteY, 0x19),
            ],
        ),
        (
            "asl",
            &[
                (Mode::ZeroPage, 0x06),
                (Mode::Absolute, 0x0E),
                (Mode::ZeroPageX, 0x16),
                (Mode::AbsoluteX, 0x1E),
            ],
        ),
        ("bit", &[(Mode::ZeroPage, 0x24), (Mode::Absolute, 0x2C)]),
        (
            "and",
            &[
                (Mode::ZeroPage, 0x25),
                (Mode::Immediate, 0x29),
                (Mode::Absolute, 0x2D),
                (Mode::ZeroPageX, 0x35),
                (Mode::AbsoluteX, 0x3D),
                (Mode::AbsoluteY, 0x39),
            ],
        ),
        (
            "rol",
            &[
                (Mode::ZeroPage, 0x26),
                (Mode::Absolute, 0x2E),
                (Mode::ZeroPageX, 0x36),
                (Mode::AbsoluteX, 0x3E),
            ],
        ),
        (
            "eor",
            &[
                (Mode::ZeroPage, 0x45),
                (Mode::Immediate, 0x49),
                (Mode::Absolute, 0x4D),
                (Mode::ZeroPageX, 0x55),
                (Mode::AbsoluteX, 0x5D),
                (Mode::AbsoluteY, 0x59),
            ],
        ),
        (
            "lsr",
            &[
                (Mode::ZeroPage, 0x46),
                (Mode::Absolute, 0x4E),
                (Mode::ZeroPageX, 0x56),
                (Mode::AbsoluteX, 0x5E),
            ],
        ),
        (
            "adc",
            &[
                (Mode::ZeroPage, 0x65),
                (Mode::Immediate, 0x69),
                (Mode::Absolute, 0x6D),
                (Mode::ZeroPageX, 0x75),
                (Mode::AbsoluteX, 0x7D),
                (Mode::AbsoluteY, 0x79),
            ],
        ),
        (
            "ror",
            &[
                (Mode::ZeroPage, 0x66),
                (Mode::Absolute, 0x6E),
                (Mode::ZeroPageX, 0x76),
                (Mode::AbsoluteX, 0x7E),
            ],
        ),
        (
            "sty",
            &[
                (Mode::ZeroPage, 0x84),
                (Mode::Absolute, 0x8C),
                (Mode::ZeroPageX, 0x94),
            ],
        ),
        (
            "stx",
            &[
                (Mode::ZeroPage, 0x86),
                (Mode::Absolute, 0x8E),
                (Mode::ZeroPageY, 0x96),
            ],
        ),
        (
            "sta",
            &[
                (Mode::ZeroPage, 0x85),
                (Mode::Absolute, 0x8D),
                (Mode::ZeroPageX, 0x95),
                (Mode::AbsoluteX, 0x9D),
                (Mode::AbsoluteY, 0x99),
            ],
        ),
        (
            "ldy",
            &[
                (Mode::Immediate, 0xA0),
                (Mode::ZeroPage, 0xA4),
                (Mode::Absolute, 0xAC),
                (Mode::ZeroPageX, 0xB4),
                (Mode::AbsoluteX, 0xBC),
            ],
        ),
        (
            "ldx",
            &[
                (Mode::Immediate, 0xA2),
                (Mode::ZeroPage, 0xA6),
                (Mode::Absolute, 0xAE),
                (Mode::ZeroPageY, 0xB6),
                (Mode::AbsoluteY, 0xBE),
            ],
        ),
        (
            "lda",
            &[
                (Mode::Immediate, 0xA9),
                (Mode::ZeroPage, 0xA5),
                (Mode::Absolute, 0xAD),
                (Mode::ZeroPageX, 0xB5),
                (Mode::AbsoluteX, 0xBD),
                (Mode::AbsoluteY, 0xB9),
            ],
        ),
        (
            "cpy",
            &[
                (Mode::Immediate, 0xC0),
                (Mode::ZeroPage, 0xC4),
                (Mode::Absolute, 0xCC),
            ],
        ),
        (
            "cmp",
            &[
                (Mode::Immediate, 0xC9),
                (Mode::ZeroPage, 0xC5),
                (Mode::Absolute, 0xCD),
                (Mode::ZeroPageX, 0xD5),
                (Mode::AbsoluteX, 0xDD),
                (Mode::AbsoluteY, 0xD9),
            ],
        ),
        (
            "dec",
            &[
                (Mode::ZeroPage, 0xC6),
                (Mode::Absolute, 0xCE),
                (Mode::ZeroPageX, 0xD6),
                (Mode::AbsoluteX, 0xDE),
            ],
        ),
        (
            "cpx",
            &[
                (Mode::Immediate, 0xE0),
                (Mode::ZeroPage, 0xE4),
                (Mode::Absolute, 0xEC),
            ],
        ),
        (
            "sbc",
            &[
                (Mode::Immediate, 0xE9),
                (Mode::ZeroPage, 0xE5),
                (Mode::Absolute, 0xED),
                (Mode::ZeroPageX, 0xF5),
                (Mode::AbsoluteX, 0xFD),
                (Mode::AbsoluteY, 0xF9),
            ],
        ),
        (
            "inc",
            &[
                (Mode::ZeroPage, 0xE6),
                (Mode::Absolute, 0xEE),
                (Mode::ZeroPageX, 0xF6),
                (Mode::AbsoluteX, 0xFE),
            ],
        ),
    ];
    let base = rows
        .iter()
        .find(|(name, _)| *name == m)
        .and_then(|(_, modes)| {
            modes
                .iter()
                .find_map(|(candidate, code)| (*candidate == mode).then_some(*code))
        });
    if base.is_some() {
        return base;
    }

    if variant == Mos6502Variant::Cmos65C02 || variant == Mos6502Variant::Wdc65C816 {
        return opcode_65c02_group(m, mode);
    }
    None
}

fn opcode_65c02_group(m: &str, mode: Mode) -> Option<u8> {
    match (m, mode) {
        ("stz", Mode::ZeroPage) => Some(0x64),
        ("stz", Mode::Absolute) => Some(0x9C),
        ("stz", Mode::ZeroPageX) => Some(0x74),
        ("stz", Mode::AbsoluteX) => Some(0x9E),
        ("trb", Mode::ZeroPage) => Some(0x14),
        ("trb", Mode::Absolute) => Some(0x1C),
        ("tsb", Mode::ZeroPage) => Some(0x04),
        ("tsb", Mode::Absolute) => Some(0x0C),
        // RMB0-7: bit reset, zero page
        ("rmb0", Mode::ZeroPage) => Some(0x07),
        ("rmb1", Mode::ZeroPage) => Some(0x17),
        ("rmb2", Mode::ZeroPage) => Some(0x27),
        ("rmb3", Mode::ZeroPage) => Some(0x37),
        ("rmb4", Mode::ZeroPage) => Some(0x47),
        ("rmb5", Mode::ZeroPage) => Some(0x57),
        ("rmb6", Mode::ZeroPage) => Some(0x67),
        ("rmb7", Mode::ZeroPage) => Some(0x77),
        // SMB0-7: bit set, zero page
        ("smb0", Mode::ZeroPage) => Some(0x87),
        ("smb1", Mode::ZeroPage) => Some(0x97),
        ("smb2", Mode::ZeroPage) => Some(0xA7),
        ("smb3", Mode::ZeroPage) => Some(0xB7),
        ("smb4", Mode::ZeroPage) => Some(0xC7),
        ("smb5", Mode::ZeroPage) => Some(0xD7),
        ("smb6", Mode::ZeroPage) => Some(0xE7),
        ("smb7", Mode::ZeroPage) => Some(0xF7),
        _ => None,
    }
}

fn is_bit_branch(m: &str) -> bool {
    matches!(
        m,
        "bbr0"
            | "bbr1"
            | "bbr2"
            | "bbr3"
            | "bbr4"
            | "bbr5"
            | "bbr6"
            | "bbr7"
            | "bbs0"
            | "bbs1"
            | "bbs2"
            | "bbs3"
            | "bbs4"
            | "bbs5"
            | "bbs6"
            | "bbs7"
    )
}

fn bbr_bbs_opcode(m: &str, zp: u32, target: u32, pc: u32) -> Option<Vec<u8>> {
    let opcode = match m {
        "bbr0" => Some(0x0Fu8),
        "bbr1" => Some(0x1F),
        "bbr2" => Some(0x2F),
        "bbr3" => Some(0x3F),
        "bbr4" => Some(0x4F),
        "bbr5" => Some(0x5F),
        "bbr6" => Some(0x6F),
        "bbr7" => Some(0x7F),
        "bbs0" => Some(0x8Fu8),
        "bbs1" => Some(0x9F),
        "bbs2" => Some(0xAF),
        "bbs3" => Some(0xBF),
        "bbs4" => Some(0xCF),
        "bbs5" => Some(0xDF),
        "bbs6" => Some(0xEF),
        "bbs7" => Some(0xFF),
        _ => None,
    }?;
    let rel = relative_offset_6502(pc.wrapping_add(1), target).ok()?;
    Some(vec![opcode, zp as u8, rel])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(text: &str, variant: Mos6502Variant) -> Vec<u8> {
        encode(text, &HashMap::new(), 0, false, variant).unwrap()
    }

    fn enc_at(text: &str, pc: u32, variant: Mos6502Variant) -> Vec<u8> {
        encode(text, &HashMap::new(), pc, false, variant).unwrap()
    }

    fn err(text: &str, variant: Mos6502Variant) -> String {
        encode(text, &HashMap::new(), 0, false, variant)
            .unwrap_err()
            .to_string()
    }

    // ── NMOS 6502 (baseline) ──────────────────────────────────────────────

    #[test]
    fn nmos_standard_opcodes() {
        assert_eq!(enc("lda #$10", Mos6502Variant::Nmos6502), vec![0xA9, 0x10]);
        assert_eq!(enc("lda $20", Mos6502Variant::Nmos6502), vec![0xA5, 0x20]);
        assert_eq!(
            enc("lda $2000", Mos6502Variant::Nmos6502),
            vec![0xAD, 0x00, 0x20]
        );
        assert_eq!(enc("lda $20,x", Mos6502Variant::Nmos6502), vec![0xB5, 0x20]);
        assert_eq!(
            enc("lda $2000,x", Mos6502Variant::Nmos6502),
            vec![0xBD, 0x00, 0x20]
        );
        assert_eq!(
            enc("lda $2000,y", Mos6502Variant::Nmos6502),
            vec![0xB9, 0x00, 0x20]
        );
        assert_eq!(
            enc("lda ($20,x)", Mos6502Variant::Nmos6502),
            vec![0xA1, 0x20]
        );
        assert_eq!(
            enc("lda ($20),y", Mos6502Variant::Nmos6502),
            vec![0xB1, 0x20]
        );
        assert_eq!(
            enc("sta $1234", Mos6502Variant::Nmos6502),
            vec![0x8D, 0x34, 0x12]
        );
        assert_eq!(
            enc("jmp $3456", Mos6502Variant::Nmos6502),
            vec![0x4C, 0x56, 0x34]
        );
        assert_eq!(
            enc("jsr $5678", Mos6502Variant::Nmos6502),
            vec![0x20, 0x78, 0x56]
        );
        assert_eq!(enc("beq $10", Mos6502Variant::Nmos6502), vec![0xF0, 0x0E]);
        assert_eq!(enc("nop", Mos6502Variant::Nmos6502), vec![0xEA]);
        assert_eq!(enc("sbc #$05", Mos6502Variant::Nmos6502), vec![0xE9, 0x05]);
    }

    #[test]
    fn nmos_accumulator_and_implied() {
        assert_eq!(enc("asl a", Mos6502Variant::Nmos6502), vec![0x0A]);
        assert_eq!(enc("rol a", Mos6502Variant::Nmos6502), vec![0x2A]);
        assert_eq!(enc("lsr a", Mos6502Variant::Nmos6502), vec![0x4A]);
        assert_eq!(enc("ror a", Mos6502Variant::Nmos6502), vec![0x6A]);
        assert_eq!(enc("tax", Mos6502Variant::Nmos6502), vec![0xAA]);
        assert_eq!(enc("txa", Mos6502Variant::Nmos6502), vec![0x8A]);
        assert_eq!(enc("clc", Mos6502Variant::Nmos6502), vec![0x18]);
        assert_eq!(enc("sec", Mos6502Variant::Nmos6502), vec![0x38]);
    }

    #[test]
    fn nmos_relative_branch() {
        // bcs $10 at PC=0 → offset = $10 - 2 = $0E
        assert_eq!(
            enc_at("bcs $10", 0, Mos6502Variant::Nmos6502),
            vec![0xB0, 0x0E]
        );
        // bne label at PC=0x100, target = 0x0F8 → offset = 0x0F8 - (0x100 + 2) = -10 = 0xF6
        let mut labels = HashMap::new();
        labels.insert("label".to_string(), 0x0F8);
        assert_eq!(
            encode("bne label", &labels, 0x100, true, Mos6502Variant::Nmos6502).unwrap(),
            vec![0xD0, 0xF6]
        );
    }

    // ── 65C02 opcodes ─────────────────────────────────────────────────────

    #[test]
    fn cmos_bra() {
        assert_eq!(
            enc_at("bra $10", 0, Mos6502Variant::Cmos65C02),
            vec![0x80, 0x0E]
        );
        assert_eq!(
            enc_at("bra $05", 2, Mos6502Variant::Cmos65C02),
            vec![0x80, 0x01]
        );
        assert_eq!(
            enc_at("bra $02", 0, Mos6502Variant::Cmos65C02),
            vec![0x80, 0x00]
        );
    }

    #[test]
    fn cmos_stack_ops() {
        assert_eq!(enc("phx", Mos6502Variant::Cmos65C02), vec![0xDA]);
        assert_eq!(enc("phy", Mos6502Variant::Cmos65C02), vec![0x5A]);
        assert_eq!(enc("plx", Mos6502Variant::Cmos65C02), vec![0xFA]);
        assert_eq!(enc("ply", Mos6502Variant::Cmos65C02), vec![0x7A]);
    }

    #[test]
    fn cmos_stp_wai() {
        assert_eq!(enc("stp", Mos6502Variant::Cmos65C02), vec![0xDB]);
        assert_eq!(enc("wai", Mos6502Variant::Cmos65C02), vec![0xCB]);
    }

    #[test]
    fn cmos_inc_dec_accumulator() {
        assert_eq!(enc("inc a", Mos6502Variant::Cmos65C02), vec![0x1A]);
        assert_eq!(enc("dec a", Mos6502Variant::Cmos65C02), vec![0x3A]);
    }

    #[test]
    fn cmos_bit_immediate() {
        assert_eq!(enc("bit #$80", Mos6502Variant::Cmos65C02), vec![0x89, 0x80]);
        assert_eq!(enc("bit #$00", Mos6502Variant::Cmos65C02), vec![0x89, 0x00]);
    }

    #[test]
    fn cmos_stz() {
        assert_eq!(enc("stz $10", Mos6502Variant::Cmos65C02), vec![0x64, 0x10]);
        assert_eq!(
            enc("stz $1234", Mos6502Variant::Cmos65C02),
            vec![0x9C, 0x34, 0x12]
        );
        assert_eq!(
            enc("stz $10,x", Mos6502Variant::Cmos65C02),
            vec![0x74, 0x10]
        );
        assert_eq!(
            enc("stz $1234,x", Mos6502Variant::Cmos65C02),
            vec![0x9E, 0x34, 0x12]
        );
    }

    #[test]
    fn cmos_variant_rejects_on_nmos() {
        assert!(err("bra $10", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("phx", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("phy", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("plx", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("ply", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("stp", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("wai", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("inc a", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("dec a", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("bit #$80", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("stz $10", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("trb $20", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("tsb $30", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    }

    // ── 65C02 JMP (abs,X) ────────────────────────────────────────────────

    #[test]
    fn cmos_jmp_indexed_indirect_x() {
        assert_eq!(
            enc("jmp ($1234,x)", Mos6502Variant::Cmos65C02),
            vec![0x7C, 0x34, 0x12]
        );
    }

    #[test]
    fn cmos_jmp_indexed_indirect_x_rejected_on_nmos() {
        assert!(
            err("jmp ($1234,x)", Mos6502Variant::Nmos6502).contains("assembler does not support")
        );
    }

    // ── 65C02 (zp) indirect ───────────────────────────────────────────────

    #[test]
    fn cmos_zp_indirect_adc() {
        assert_eq!(
            enc("adc ($12)", Mos6502Variant::Cmos65C02),
            vec![0x72, 0x12]
        );
    }

    #[test]
    fn cmos_zp_indirect_and_cmp_ora() {
        assert_eq!(
            enc("and ($34)", Mos6502Variant::Cmos65C02),
            vec![0x32, 0x34]
        );
        assert_eq!(
            enc("cmp ($56)", Mos6502Variant::Cmos65C02),
            vec![0xD2, 0x56]
        );
        assert_eq!(
            enc("ora ($78)", Mos6502Variant::Cmos65C02),
            vec![0x12, 0x78]
        );
    }

    #[test]
    fn cmos_zp_indirect_eor_lda_sta_sbc() {
        assert_eq!(
            enc("eor ($10)", Mos6502Variant::Cmos65C02),
            vec![0x52, 0x10]
        );
        assert_eq!(
            enc("lda ($20)", Mos6502Variant::Cmos65C02),
            vec![0xB2, 0x20]
        );
        assert_eq!(
            enc("sta ($30)", Mos6502Variant::Cmos65C02),
            vec![0x92, 0x30]
        );
        assert_eq!(
            enc("sbc ($40)", Mos6502Variant::Cmos65C02),
            vec![0xF2, 0x40]
        );
    }

    #[test]
    fn cmos_zp_indirect_works_on_65c816_too() {
        assert_eq!(
            enc("adc ($12)", Mos6502Variant::Wdc65C816),
            vec![0x72, 0x12]
        );
        assert_eq!(
            enc("lda ($20)", Mos6502Variant::Wdc65C816),
            vec![0xB2, 0x20]
        );
    }

    #[test]
    fn cmos_zp_indirect_rejected_on_nmos() {
        assert!(err("adc ($12)", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("sta ($30)", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    }

    // ── 65C02 RMB / SMB ─────────────────────────────────────────────────

    #[test]
    fn cmos_rmb_smb() {
        assert_eq!(enc("rmb0 $12", Mos6502Variant::Cmos65C02), vec![0x07, 0x12]);
        assert_eq!(enc("rmb3 $34", Mos6502Variant::Cmos65C02), vec![0x37, 0x34]);
        assert_eq!(enc("rmb7 $56", Mos6502Variant::Cmos65C02), vec![0x77, 0x56]);
        assert_eq!(enc("smb0 $78", Mos6502Variant::Cmos65C02), vec![0x87, 0x78]);
        assert_eq!(enc("smb4 $9A", Mos6502Variant::Cmos65C02), vec![0xC7, 0x9A]);
        assert_eq!(enc("smb7 $BC", Mos6502Variant::Cmos65C02), vec![0xF7, 0xBC]);
    }

    #[test]
    fn cmos_rmb_smb_rejected_on_nmos() {
        assert!(err("rmb0 $12", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("smb0 $12", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    }

    // ── 65C02 BBR / BBS ─────────────────────────────────────────────────

    #[test]
    fn cmos_bbr_bbs() {
        // relative offset = target - (pc + 3) for 3-byte instruction
        assert_eq!(
            enc_at("bbr0 $12,$20", 0, Mos6502Variant::Cmos65C02),
            vec![0x0F, 0x12, 0x1D]
        );
        assert_eq!(
            enc_at("bbr4 $34,$50", 0, Mos6502Variant::Cmos65C02),
            vec![0x4F, 0x34, 0x4D]
        );
        assert_eq!(
            enc_at("bbs0 $56,$80", 0, Mos6502Variant::Cmos65C02),
            vec![0x8F, 0x56, 0x7D]
        );
        assert_eq!(
            enc_at("bbs7 $78,$60", 0, Mos6502Variant::Cmos65C02),
            vec![0xFF, 0x78, 0x5D]
        );
    }

    #[test]
    fn cmos_bbr_bbs_rejected_on_nmos() {
        assert!(err("bbr0 $12", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("bbs0 $12", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    }

    // ── 65C816 opcodes ────────────────────────────────────────────────────

    #[test]
    fn wdc816_exchange() {
        assert_eq!(enc("xba", Mos6502Variant::Wdc65C816), vec![0xEB]);
        assert_eq!(enc("xce", Mos6502Variant::Wdc65C816), vec![0xFB]);
    }

    #[test]
    fn wdc816_rep_sep() {
        assert_eq!(enc("rep #$10", Mos6502Variant::Wdc65C816), vec![0xC2, 0x10]);
        assert_eq!(enc("sep #$80", Mos6502Variant::Wdc65C816), vec![0xE2, 0x80]);
    }

    #[test]
    fn wdc816_block_move() {
        // MVP/MVN take dst_bank,src_bank as two-byte operand (LE)
        // mvp $00,$01 → src=$00, dst=$01 → packed 0x0001 → LE [0x01,0x00]
        assert_eq!(
            enc("mvp $00,$01", Mos6502Variant::Wdc65C816),
            vec![0x44, 0x01, 0x00]
        );
        assert_eq!(
            enc("mvn $FF,$80", Mos6502Variant::Wdc65C816),
            vec![0x54, 0x80, 0xFF]
        );
    }

    #[test]
    fn wdc816_push_effective() {
        assert_eq!(
            enc("pea $1234", Mos6502Variant::Wdc65C816),
            vec![0xF4, 0x34, 0x12]
        );
        assert_eq!(
            enc("pei ($10)", Mos6502Variant::Wdc65C816),
            vec![0xD4, 0x10]
        );
    }

    #[test]
    fn wdc816_per() {
        // per $10 at PC=0 → offset = $10 - (0 + 3) = $0D (16-bit)
        assert_eq!(
            enc_at("per $10", 0, Mos6502Variant::Wdc65C816),
            vec![0x62, 0x0D, 0x00]
        );
        assert_eq!(
            enc_at("per $05", 0, Mos6502Variant::Wdc65C816),
            vec![0x62, 0x02, 0x00]
        );
    }

    #[test]
    fn wdc816_indexed_indirect_x() {
        assert_eq!(
            enc("jmp ($1234,x)", Mos6502Variant::Wdc65C816),
            vec![0x7C, 0x34, 0x12]
        );
        assert_eq!(
            enc("jsr ($5678,x)", Mos6502Variant::Wdc65C816),
            vec![0xFC, 0x78, 0x56]
        );
    }

    #[test]
    fn wdc816_absolute_long_jump() {
        assert_eq!(
            enc("jmp !$123456", Mos6502Variant::Wdc65C816),
            vec![0x5C, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            enc("jsr !$654321", Mos6502Variant::Wdc65C816),
            vec![0x22, 0x21, 0x43, 0x65]
        );
    }

    #[test]
    fn wdc816_indirect_long_jmp() {
        assert_eq!(
            enc("jmp [$1234]", Mos6502Variant::Wdc65C816),
            vec![0xDC, 0x34, 0x12]
        );
    }

    #[test]
    fn wdc816_absolute_long_alu() {
        assert_eq!(
            enc("lda !$123456", Mos6502Variant::Wdc65C816),
            vec![0xAF, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            enc("sta !$123456", Mos6502Variant::Wdc65C816),
            vec![0x8F, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            enc("adc !$123456", Mos6502Variant::Wdc65C816),
            vec![0x6F, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            enc("sbc !$123456", Mos6502Variant::Wdc65C816),
            vec![0xEF, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            enc("and !$123456", Mos6502Variant::Wdc65C816),
            vec![0x2F, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            enc("ora !$123456", Mos6502Variant::Wdc65C816),
            vec![0x0F, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            enc("eor !$123456", Mos6502Variant::Wdc65C816),
            vec![0x4F, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            enc("cmp !$123456", Mos6502Variant::Wdc65C816),
            vec![0xCF, 0x56, 0x34, 0x12]
        );
    }

    #[test]
    fn wdc816_auto_long_for_sufficiently_large_address() {
        // Without `!`, values > 0xFFFF should use AbsoluteLong
        assert_eq!(
            enc("lda $123456", Mos6502Variant::Wdc65C816),
            vec![0xAF, 0x56, 0x34, 0x12]
        );
        assert_eq!(
            enc("sta $123456", Mos6502Variant::Wdc65C816),
            vec![0x8F, 0x56, 0x34, 0x12]
        );
        // Values ≤ 0xFFFF still use normal absolute
        assert_eq!(
            enc("lda $1234", Mos6502Variant::Wdc65C816),
            vec![0xAD, 0x34, 0x12]
        );
    }

    #[test]
    fn wdc816_wdm() {
        assert_eq!(enc("wdm #$42", Mos6502Variant::Wdc65C816), vec![0x42, 0x42]);
        assert_eq!(enc("wdm #$00", Mos6502Variant::Wdc65C816), vec![0x42, 0x00]);
    }

    #[test]
    fn wdc816_cmos_opcodes_also_work() {
        // 65C816 is a superset of 65C02
        assert_eq!(enc("bra $10", Mos6502Variant::Wdc65C816), vec![0x80, 0x0E]);
        assert_eq!(enc("phx", Mos6502Variant::Wdc65C816), vec![0xDA]);
        assert_eq!(enc("phy", Mos6502Variant::Wdc65C816), vec![0x5A]);
        assert_eq!(enc("plx", Mos6502Variant::Wdc65C816), vec![0xFA]);
        assert_eq!(enc("ply", Mos6502Variant::Wdc65C816), vec![0x7A]);
        assert_eq!(enc("stp", Mos6502Variant::Wdc65C816), vec![0xDB]);
        assert_eq!(enc("wai", Mos6502Variant::Wdc65C816), vec![0xCB]);
        assert_eq!(enc("inc a", Mos6502Variant::Wdc65C816), vec![0x1A]);
        assert_eq!(enc("dec a", Mos6502Variant::Wdc65C816), vec![0x3A]);
        assert_eq!(enc("bit #$80", Mos6502Variant::Wdc65C816), vec![0x89, 0x80]);
        assert_eq!(enc("stz $10", Mos6502Variant::Wdc65C816), vec![0x64, 0x10]);
        assert_eq!(enc("trb $20", Mos6502Variant::Wdc65C816), vec![0x14, 0x20]);
        assert_eq!(enc("tsb $30", Mos6502Variant::Wdc65C816), vec![0x04, 0x30]);
    }

    #[test]
    fn wdc816_variant_rejects_on_nmos() {
        assert!(err("xba", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("xce", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("rep #$01", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("sep #$01", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("pei ($10)", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    }

    // ── 65C816 COP, BRL, RTL ──────────────────────────────────────────────

    #[test]
    fn wdc816_cop() {
        assert_eq!(enc("cop #$42", Mos6502Variant::Wdc65C816), vec![0x02, 0x42]);
        assert_eq!(enc("cop #$00", Mos6502Variant::Wdc65C816), vec![0x02, 0x00]);
    }

    #[test]
    fn wdc816_brl() {
        // brl $10 at PC=0 → offset = $10 - (0 + 3) = $0D (16-bit)
        assert_eq!(
            enc_at("brl $10", 0, Mos6502Variant::Wdc65C816),
            vec![0x82, 0x0D, 0x00]
        );
        assert_eq!(
            enc_at("brl $0120", 0, Mos6502Variant::Wdc65C816),
            vec![0x82, 0x1D, 0x01]
        );
    }

    #[test]
    fn wdc816_rtl() {
        assert_eq!(enc("rtl", Mos6502Variant::Wdc65C816), vec![0x6B]);
    }

    // ── 65C816 bank/DP register push/pull ─────────────────────────────────

    #[test]
    fn wdc816_phb_phd_phk() {
        assert_eq!(enc("phb", Mos6502Variant::Wdc65C816), vec![0x8B]);
        assert_eq!(enc("phd", Mos6502Variant::Wdc65C816), vec![0x0B]);
        assert_eq!(enc("phk", Mos6502Variant::Wdc65C816), vec![0x4B]);
    }

    #[test]
    fn wdc816_plb_pld() {
        assert_eq!(enc("plb", Mos6502Variant::Wdc65C816), vec![0xAB]);
        assert_eq!(enc("pld", Mos6502Variant::Wdc65C816), vec![0x2B]);
    }

    // ── 65C816 transfer instructions ──────────────────────────────────────

    #[test]
    fn wdc816_transfer_tcs_tsc_tcd_tdc() {
        assert_eq!(enc("tcs", Mos6502Variant::Wdc65C816), vec![0x1B]);
        assert_eq!(enc("tsc", Mos6502Variant::Wdc65C816), vec![0x3B]);
        assert_eq!(enc("tcd", Mos6502Variant::Wdc65C816), vec![0x5B]);
        assert_eq!(enc("tdc", Mos6502Variant::Wdc65C816), vec![0x7B]);
    }

    #[test]
    fn wdc816_transfer_txy_tyx() {
        assert_eq!(enc("txy", Mos6502Variant::Wdc65C816), vec![0x9B]);
        assert_eq!(enc("tyx", Mos6502Variant::Wdc65C816), vec![0xBB]);
    }

    #[test]
    fn wdc816_new_implied_rejected_on_nmos() {
        assert!(err("rtl", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("phb", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("phd", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("phk", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("plb", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("pld", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("tcs", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("tsc", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("tcd", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("tdc", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("txy", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("tyx", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("cop #$01", Mos6502Variant::Nmos6502).contains("assembler does not support"));
        assert!(err("brl $10", Mos6502Variant::Nmos6502).contains("assembler does not support"));
    }

    // ── Ricoh 2A03 ────────────────────────────────────────────────────────

    #[test]
    fn ricoh_sbc_immediate_uses_eb() {
        // NMOS uses $E9, Ricoh 2A03 uses $EB
        assert_eq!(enc("sbc #$05", Mos6502Variant::Ricoh2A03), vec![0xEB, 0x05]);
        assert_eq!(enc("sbc #$FF", Mos6502Variant::Ricoh2A03), vec![0xEB, 0xFF]);
    }

    #[test]
    fn ricoh_standard_nmos_opcodes_still_work() {
        assert_eq!(enc("lda #$10", Mos6502Variant::Ricoh2A03), vec![0xA9, 0x10]);
        assert_eq!(
            enc("sta $2000", Mos6502Variant::Ricoh2A03),
            vec![0x8D, 0x00, 0x20]
        );
        assert_eq!(
            enc("jsr $2000", Mos6502Variant::Ricoh2A03),
            vec![0x20, 0x00, 0x20]
        );
        assert_eq!(enc("nop", Mos6502Variant::Ricoh2A03), vec![0xEA]);
        assert_eq!(enc("tax", Mos6502Variant::Ricoh2A03), vec![0xAA]);
    }

    #[test]
    fn ricoh_rejects_65c02_opcodes() {
        assert!(err("bra $10", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
        assert!(err("phx", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
        assert!(err("stz $10", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
        assert!(err("inc a", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
    }

    #[test]
    fn ricoh_rejects_65c816_opcodes() {
        assert!(err("xba", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
        assert!(err("xce", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
        assert!(err("rep #$01", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
        assert!(err("sep #$01", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
        assert!(err("pei ($10)", Mos6502Variant::Ricoh2A03).contains("assembler does not support"));
    }

    // ── instruction_len_for_variant ───────────────────────────────────────

    #[test]
    fn test_instruction_len_for_variant() {
        assert_eq!(
            instruction_len_for_variant("nop", Mos6502Variant::Nmos6502).unwrap(),
            1
        );
        assert_eq!(
            instruction_len_for_variant("nop", Mos6502Variant::Cmos65C02).unwrap(),
            1
        );
        assert_eq!(
            instruction_len_for_variant("nop", Mos6502Variant::Wdc65C816).unwrap(),
            1
        );
        assert_eq!(
            instruction_len_for_variant("nop", Mos6502Variant::Ricoh2A03).unwrap(),
            1
        );

        assert_eq!(
            instruction_len_for_variant("bra $10", Mos6502Variant::Cmos65C02).unwrap(),
            2
        );
        assert_eq!(
            instruction_len_for_variant("bra $10", Mos6502Variant::Wdc65C816).unwrap(),
            2
        );

        assert_eq!(
            instruction_len_for_variant("phx", Mos6502Variant::Cmos65C02).unwrap(),
            1
        );
        assert_eq!(
            instruction_len_for_variant("phx", Mos6502Variant::Wdc65C816).unwrap(),
            1
        );

        assert_eq!(
            instruction_len_for_variant("lda !$123456", Mos6502Variant::Wdc65C816).unwrap(),
            4
        );
        assert_eq!(
            instruction_len_for_variant("jmp !$123456", Mos6502Variant::Wdc65C816).unwrap(),
            4
        );
        assert_eq!(
            instruction_len_for_variant("per $10", Mos6502Variant::Wdc65C816).unwrap(),
            3
        );
        assert_eq!(
            instruction_len_for_variant("pea $1234", Mos6502Variant::Wdc65C816).unwrap(),
            3
        );

        assert_eq!(
            instruction_len_for_variant("sbc #$05", Mos6502Variant::Ricoh2A03).unwrap(),
            2
        );
    }

    // ── encode_instruction_for_variant ────────────────────────────────────

    #[test]
    fn test_encode_instruction_for_variant_with_labels() {
        let mut labels = HashMap::new();
        labels.insert("target".to_string(), 0x2000);
        let bytes = encode_instruction_for_variant(
            "jmp target",
            &labels,
            0x1000,
            true,
            Mos6502Variant::Nmos6502,
        )
        .unwrap();
        assert_eq!(bytes, vec![0x4C, 0x00, 0x20]);

        let bytes = encode_instruction_for_variant(
            "beq target",
            &labels,
            0x1FF0,
            true,
            Mos6502Variant::Nmos6502,
        )
        .unwrap();
        // offset = 0x2000 - (0x1FF0 + 2) = 0x2000 - 0x1FF2 = 0x0E
        assert_eq!(bytes, vec![0xF0, 0x0E]);
    }

    #[test]
    fn test_encode_instruction_for_variant_defaults_to_nmos() {
        let bytes = encode_instruction("lda #$10", &HashMap::new(), 0, false).unwrap();
        assert_eq!(bytes, vec![0xA9, 0x10]);
    }

    // ── Variant-as-str ────────────────────────────────────────────────────

    #[test]
    fn variant_as_str() {
        assert_eq!(Mos6502Variant::Nmos6502.as_str(), "6502");
        assert_eq!(Mos6502Variant::Cmos65C02.as_str(), "65C02");
        assert_eq!(Mos6502Variant::Wdc65C816.as_str(), "65C816");
        assert_eq!(Mos6502Variant::Ricoh2A03.as_str(), "2A03");
    }
}
