use std::collections::HashMap;

use crate::diagnostic::Diagnostic;
use crate::vm::{parse_number, relative_offset};

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
    IndexedIndirect,
    IndirectIndexed,
    Relative,
}

pub fn instruction_len(text: &str) -> Result<usize, Diagnostic> {
    Ok(encode(text, &HashMap::new(), 0, false)?.len())
}

pub fn encode_instruction(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    encode(text, labels, pc, resolve)
}

fn encode(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let normalized = text.trim().to_ascii_lowercase();
    let (mnemonic, operand) = normalized
        .split_once(char::is_whitespace)
        .map_or((normalized.as_str(), ""), |(op, rest)| (op, rest.trim()));
    let (mode, value) = parse_operand(mnemonic, operand, labels, pc, resolve)?;
    let opcode = opcode(mnemonic, mode).ok_or_else(|| {
        Diagnostic::new(format!(
            "assembler does not support 6502 instruction `{text}`"
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
        Mode::Relative => out.push(relative_offset(pc, value)?),
        Mode::Absolute | Mode::AbsoluteX | Mode::AbsoluteY | Mode::Indirect => {
            push16(&mut out, value)?
        }
    }
    Ok(out)
}

fn parse_operand(
    mnemonic: &str,
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
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
    if is_branch(mnemonic) {
        return Ok((Mode::Relative, value_or(operand, labels, pc, resolve, 0)?));
    }
    if let Some(inner) = operand
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(",x)"))
    {
        return Ok((
            Mode::IndexedIndirect,
            value_or(inner, labels, pc, resolve, 0)?,
        ));
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
    if let Some(inner) = operand.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        return Ok((Mode::Indirect, value_or(inner, labels, pc, resolve, 0x100)?));
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
    let v = value_or(operand, labels, pc, resolve, 0x100)?;
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
    expr == "$" || expr.starts_with('$') || parse_number(expr).is_ok()
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
fn push16(out: &mut Vec<u8>, value: u32) -> Result<(), Diagnostic> {
    let v = u16::try_from(value).map_err(|_| {
        Diagnostic::new(format!("6502 address 0x{value:X} is outside 16-bit range"))
    })?;
    out.extend(v.to_le_bytes());
    Ok(())
}
fn is_branch(m: &str) -> bool {
    matches!(
        m,
        "bcc" | "bcs" | "beq" | "bmi" | "bne" | "bpl" | "bvc" | "bvs"
    )
}

fn opcode(m: &str, mode: Mode) -> Option<u8> {
    Some(match (m, mode) {
        ("brk", Mode::Implied) => 0x00,
        ("php", Mode::Implied) => 0x08,
        ("clc", Mode::Implied) => 0x18,
        ("plp", Mode::Implied) => 0x28,
        ("sec", Mode::Implied) => 0x38,
        ("rti", Mode::Implied) => 0x40,
        ("pha", Mode::Implied) => 0x48,
        ("cli", Mode::Implied) => 0x58,
        ("rts", Mode::Implied) => 0x60,
        ("pla", Mode::Implied) => 0x68,
        ("sei", Mode::Implied) => 0x78,
        ("dey", Mode::Implied) => 0x88,
        ("txa", Mode::Implied) => 0x8A,
        ("tya", Mode::Implied) => 0x98,
        ("txs", Mode::Implied) => 0x9A,
        ("tay", Mode::Implied) => 0xA8,
        ("tax", Mode::Implied) => 0xAA,
        ("clv", Mode::Implied) => 0xB8,
        ("tsx", Mode::Implied) => 0xBA,
        ("iny", Mode::Implied) => 0xC8,
        ("dex", Mode::Implied) => 0xCA,
        ("cld", Mode::Implied) => 0xD8,
        ("inx", Mode::Implied) => 0xE8,
        ("nop", Mode::Implied) => 0xEA,
        ("sed", Mode::Implied) => 0xF8,
        ("asl", Mode::Accumulator) => 0x0A,
        ("rol", Mode::Accumulator) => 0x2A,
        ("lsr", Mode::Accumulator) => 0x4A,
        ("ror", Mode::Accumulator) => 0x6A,
        ("jsr", Mode::Absolute) => 0x20,
        ("jmp", Mode::Absolute) => 0x4C,
        ("jmp", Mode::Indirect) => 0x6C,
        ("bpl", Mode::Relative) => 0x10,
        ("bmi", Mode::Relative) => 0x30,
        ("bvc", Mode::Relative) => 0x50,
        ("bvs", Mode::Relative) => 0x70,
        ("bcc", Mode::Relative) => 0x90,
        ("bcs", Mode::Relative) => 0xB0,
        ("bne", Mode::Relative) => 0xD0,
        ("beq", Mode::Relative) => 0xF0,
        ("ora", Mode::IndexedIndirect) => 0x01,
        ("and", Mode::IndexedIndirect) => 0x21,
        ("eor", Mode::IndexedIndirect) => 0x41,
        ("adc", Mode::IndexedIndirect) => 0x61,
        ("sta", Mode::IndexedIndirect) => 0x81,
        ("lda", Mode::IndexedIndirect) => 0xA1,
        ("cmp", Mode::IndexedIndirect) => 0xC1,
        ("sbc", Mode::IndexedIndirect) => 0xE1,
        ("ora", Mode::IndirectIndexed) => 0x11,
        ("and", Mode::IndirectIndexed) => 0x31,
        ("eor", Mode::IndirectIndexed) => 0x51,
        ("adc", Mode::IndirectIndexed) => 0x71,
        ("sta", Mode::IndirectIndexed) => 0x91,
        ("lda", Mode::IndirectIndexed) => 0xB1,
        ("cmp", Mode::IndirectIndexed) => 0xD1,
        ("sbc", Mode::IndirectIndexed) => 0xF1,
        _ => return opcode_group(m, mode),
    })
}

fn opcode_group(m: &str, mode: Mode) -> Option<u8> {
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
    rows.iter()
        .find(|(name, _)| *name == m)
        .and_then(|(_, modes)| {
            modes
                .iter()
                .find_map(|(candidate, code)| (*candidate == mode).then_some(*code))
        })
}
