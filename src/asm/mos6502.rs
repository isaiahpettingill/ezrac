use std::collections::HashMap;

use crate::diagnostic::Diagnostic;
use crate::vm::parse_number;

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
        Mode::Relative => out.push(if resolve {
            relative_offset_6502(pc, value)?
        } else {
            0
        }),
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
    // Whitespace around an addressing-mode comma is insignificant, while
    // whitespace inside an expression remains available to the outer assembler.
    let normalized_operand = operand.replace(" ,", ",").replace(", ", ",");
    let operand = normalized_operand.as_str();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{target::AssemblerCpu, vm::assemble_subset_with_symbols_at};

    #[test]
    fn golden_encodes_every_official_nmos_6502_opcode_and_addressing_form() {
        // One canonical operand per documented form.  This is the 151-opcode
        // official NMOS 6502 matrix; 65C02 additions and undocumented opcodes
        // deliberately do not appear here.
        let cases: &[(&str, &[u8])] = &[
            ("brk", &[0x00]),
            ("ora ($12,x)", &[0x01, 0x12]),
            ("ora $12", &[0x05, 0x12]),
            ("asl $12", &[0x06, 0x12]),
            ("php", &[0x08]),
            ("ora #$12", &[0x09, 0x12]),
            ("asl a", &[0x0a]),
            ("ora $1234", &[0x0d, 0x34, 0x12]),
            ("asl $1234", &[0x0e, 0x34, 0x12]),
            ("bpl $1080", &[0x10, 0x7e]),
            ("ora ($12),y", &[0x11, 0x12]),
            ("ora $12,x", &[0x15, 0x12]),
            ("asl $12,x", &[0x16, 0x12]),
            ("clc", &[0x18]),
            ("ora $1234,y", &[0x19, 0x34, 0x12]),
            ("ora $1234,x", &[0x1d, 0x34, 0x12]),
            ("asl $1234,x", &[0x1e, 0x34, 0x12]),
            ("jsr $1234", &[0x20, 0x34, 0x12]),
            ("and ($12,x)", &[0x21, 0x12]),
            ("bit $12", &[0x24, 0x12]),
            ("and $12", &[0x25, 0x12]),
            ("rol $12", &[0x26, 0x12]),
            ("plp", &[0x28]),
            ("and #$12", &[0x29, 0x12]),
            ("rol a", &[0x2a]),
            ("bit $1234", &[0x2c, 0x34, 0x12]),
            ("and $1234", &[0x2d, 0x34, 0x12]),
            ("rol $1234", &[0x2e, 0x34, 0x12]),
            ("bmi $1080", &[0x30, 0x7e]),
            ("and ($12),y", &[0x31, 0x12]),
            ("and $12,x", &[0x35, 0x12]),
            ("rol $12,x", &[0x36, 0x12]),
            ("sec", &[0x38]),
            ("and $1234,y", &[0x39, 0x34, 0x12]),
            ("and $1234,x", &[0x3d, 0x34, 0x12]),
            ("rol $1234,x", &[0x3e, 0x34, 0x12]),
            ("rti", &[0x40]),
            ("eor ($12,x)", &[0x41, 0x12]),
            ("eor $12", &[0x45, 0x12]),
            ("lsr $12", &[0x46, 0x12]),
            ("pha", &[0x48]),
            ("eor #$12", &[0x49, 0x12]),
            ("lsr a", &[0x4a]),
            ("jmp $1234", &[0x4c, 0x34, 0x12]),
            ("eor $1234", &[0x4d, 0x34, 0x12]),
            ("lsr $1234", &[0x4e, 0x34, 0x12]),
            ("bvc $1080", &[0x50, 0x7e]),
            ("eor ($12),y", &[0x51, 0x12]),
            ("eor $12,x", &[0x55, 0x12]),
            ("lsr $12,x", &[0x56, 0x12]),
            ("cli", &[0x58]),
            ("eor $1234,y", &[0x59, 0x34, 0x12]),
            ("eor $1234,x", &[0x5d, 0x34, 0x12]),
            ("lsr $1234,x", &[0x5e, 0x34, 0x12]),
            ("rts", &[0x60]),
            ("adc ($12,x)", &[0x61, 0x12]),
            ("adc $12", &[0x65, 0x12]),
            ("ror $12", &[0x66, 0x12]),
            ("pla", &[0x68]),
            ("adc #$12", &[0x69, 0x12]),
            ("ror a", &[0x6a]),
            ("jmp ($1234)", &[0x6c, 0x34, 0x12]),
            ("adc $1234", &[0x6d, 0x34, 0x12]),
            ("ror $1234", &[0x6e, 0x34, 0x12]),
            ("bvs $1080", &[0x70, 0x7e]),
            ("adc ($12),y", &[0x71, 0x12]),
            ("adc $12,x", &[0x75, 0x12]),
            ("ror $12,x", &[0x76, 0x12]),
            ("sei", &[0x78]),
            ("adc $1234,y", &[0x79, 0x34, 0x12]),
            ("adc $1234,x", &[0x7d, 0x34, 0x12]),
            ("ror $1234,x", &[0x7e, 0x34, 0x12]),
            ("sta ($12,x)", &[0x81, 0x12]),
            ("sty $12", &[0x84, 0x12]),
            ("sta $12", &[0x85, 0x12]),
            ("stx $12", &[0x86, 0x12]),
            ("dey", &[0x88]),
            ("txa", &[0x8a]),
            ("sty $1234", &[0x8c, 0x34, 0x12]),
            ("sta $1234", &[0x8d, 0x34, 0x12]),
            ("stx $1234", &[0x8e, 0x34, 0x12]),
            ("bcc $1080", &[0x90, 0x7e]),
            ("sta ($12),y", &[0x91, 0x12]),
            ("sty $12,x", &[0x94, 0x12]),
            ("sta $12,x", &[0x95, 0x12]),
            ("stx $12,y", &[0x96, 0x12]),
            ("tya", &[0x98]),
            ("sta $1234,y", &[0x99, 0x34, 0x12]),
            ("txs", &[0x9a]),
            ("sta $1234,x", &[0x9d, 0x34, 0x12]),
            ("ldy #$12", &[0xa0, 0x12]),
            ("lda ($12,x)", &[0xa1, 0x12]),
            ("ldx #$12", &[0xa2, 0x12]),
            ("ldy $12", &[0xa4, 0x12]),
            ("lda $12", &[0xa5, 0x12]),
            ("ldx $12", &[0xa6, 0x12]),
            ("tay", &[0xa8]),
            ("lda #$12", &[0xa9, 0x12]),
            ("tax", &[0xaa]),
            ("ldy $1234", &[0xac, 0x34, 0x12]),
            ("lda $1234", &[0xad, 0x34, 0x12]),
            ("ldx $1234", &[0xae, 0x34, 0x12]),
            ("bcs $1080", &[0xb0, 0x7e]),
            ("lda ($12),y", &[0xb1, 0x12]),
            ("ldy $12,x", &[0xb4, 0x12]),
            ("lda $12,x", &[0xb5, 0x12]),
            ("ldx $12,y", &[0xb6, 0x12]),
            ("clv", &[0xb8]),
            ("lda $1234,y", &[0xb9, 0x34, 0x12]),
            ("tsx", &[0xba]),
            ("ldy $1234,x", &[0xbc, 0x34, 0x12]),
            ("lda $1234,x", &[0xbd, 0x34, 0x12]),
            ("ldx $1234,y", &[0xbe, 0x34, 0x12]),
            ("cpy #$12", &[0xc0, 0x12]),
            ("cmp ($12,x)", &[0xc1, 0x12]),
            ("cpy $12", &[0xc4, 0x12]),
            ("cmp $12", &[0xc5, 0x12]),
            ("dec $12", &[0xc6, 0x12]),
            ("iny", &[0xc8]),
            ("cmp #$12", &[0xc9, 0x12]),
            ("dex", &[0xca]),
            ("cpy $1234", &[0xcc, 0x34, 0x12]),
            ("cmp $1234", &[0xcd, 0x34, 0x12]),
            ("dec $1234", &[0xce, 0x34, 0x12]),
            ("bne $1080", &[0xd0, 0x7e]),
            ("cmp ($12),y", &[0xd1, 0x12]),
            ("cmp $12,x", &[0xd5, 0x12]),
            ("dec $12,x", &[0xd6, 0x12]),
            ("cld", &[0xd8]),
            ("cmp $1234,y", &[0xd9, 0x34, 0x12]),
            ("cmp $1234,x", &[0xdd, 0x34, 0x12]),
            ("dec $1234,x", &[0xde, 0x34, 0x12]),
            ("cpx #$12", &[0xe0, 0x12]),
            ("sbc ($12,x)", &[0xe1, 0x12]),
            ("cpx $12", &[0xe4, 0x12]),
            ("sbc $12", &[0xe5, 0x12]),
            ("inc $12", &[0xe6, 0x12]),
            ("inx", &[0xe8]),
            ("sbc #$12", &[0xe9, 0x12]),
            ("nop", &[0xea]),
            ("cpx $1234", &[0xec, 0x34, 0x12]),
            ("sbc $1234", &[0xed, 0x34, 0x12]),
            ("inc $1234", &[0xee, 0x34, 0x12]),
            ("beq $1080", &[0xf0, 0x7e]),
            ("sbc ($12),y", &[0xf1, 0x12]),
            ("sbc $12,x", &[0xf5, 0x12]),
            ("inc $12,x", &[0xf6, 0x12]),
            ("sed", &[0xf8]),
            ("sbc $1234,y", &[0xf9, 0x34, 0x12]),
            ("sbc $1234,x", &[0xfd, 0x34, 0x12]),
            ("inc $1234,x", &[0xfe, 0x34, 0x12]),
        ];

        let labels = HashMap::new();
        for (source, expected) in cases {
            assert_eq!(
                encode_instruction(source, &labels, 0x1000, true).unwrap(),
                *expected,
                "{source}"
            );
            assert_eq!(instruction_len(source).unwrap(), expected.len(), "{source}");
        }
        assert_eq!(cases.len(), 151, "official NMOS 6502 opcode count");
    }

    #[test]
    fn validates_branch_boundaries_and_16_bit_wraparound() {
        let labels = HashMap::new();
        for (source, expected) in [
            ("bne $0f82", vec![0xd0, 0x80]),
            ("bne $1081", vec![0xd0, 0x7f]),
            ("bne $0000", vec![0xd0, 0x00]),
            ("bne $ffff", vec![0xd0, 0xff]),
        ] {
            let pc = if source == "bne $0000" || source == "bne $ffff" {
                0xfffe
            } else {
                0x1000
            };
            assert_eq!(
                encode_instruction(source, &labels, pc, true).unwrap(),
                expected,
                "{source}"
            );
        }
        for source in ["bne $0f81", "bne $1082", "bne $10000"] {
            assert!(
                encode_instruction(source, &labels, 0x1000, true).is_err(),
                "{source}"
            );
        }
    }

    #[test]
    fn labels_are_case_insensitive_and_always_use_absolute_addressing() {
        let labels = HashMap::from([
            ("zero_page".to_owned(), 0x12),
            ("destination".to_owned(), 0x1234),
        ]);
        assert_eq!(
            encode_instruction("lda ZERO_PAGE", &labels, 0, true).unwrap(),
            [0xad, 0x12, 0x00]
        );
        assert_eq!(
            encode_instruction("lda $12", &labels, 0, true).unwrap(),
            [0xa5, 0x12]
        );
        assert_eq!(
            encode_instruction("lda ( zero_page ), y", &labels, 0, true).unwrap(),
            [0xb1, 0x12]
        );
        assert_eq!(
            encode_instruction("bne DESTINATION", &labels, 0x11b4, true).unwrap(),
            [0xd0, 0x7e]
        );

        let assembled = assemble_subset_with_symbols_at(
            AssemblerCpu::Mos6502,
            "start:\n lda target\n bne START\ntarget:\n nop\n",
            0x1000,
        )
        .unwrap();
        assert_eq!(assembled.bytes, [0xad, 0x05, 0x10, 0xd0, 0xfb, 0xea]);
    }

    #[test]
    fn rejects_65c02_undocumented_and_invalid_forms() {
        let labels = HashMap::new();
        for source in [
            // 65C02-only instructions and addressing modes.
            "bra $1000",
            "stz $12",
            "phx",
            "plx",
            "phy",
            "ply",
            "trb $12",
            "tsb $12",
            "bit #$12",
            "bit $12,x",
            "bit $1234,x",
            "jmp ($1234,x)",
            "inc a",
            "dec a",
            "wai",
            "stp",
            // Common undocumented NMOS mnemonics and unofficial NOP forms.
            "lax $12",
            "sax $12",
            "dcp $12",
            "isc $12",
            "slo $12",
            "rla $12",
            "sre $12",
            "rra $12",
            "anc #$12",
            "alr #$12",
            "arr #$12",
            "axs #$12",
            "las $1234,y",
            "ahx $1234,y",
            "tas $1234,y",
            "kil",
            "nop #$12",
            "lda ($1234,x)",
            "sta ($1234),y",
        ] {
            assert!(
                encode_instruction(source, &labels, 0x1000, true).is_err(),
                "{source}"
            );
            assert!(instruction_len(source).is_err(), "{source}");
        }
    }
}
