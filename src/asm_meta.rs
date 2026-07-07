use crate::diagnostic::Diagnostic;
use crate::target::CpuFamily;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstructionSpec {
    pub syntax: &'static str,
    pub cpus: &'static [CpuFamily],
    pub bytes: &'static [u8],
}

const Z80_AND_EZ80: &[CpuFamily] = &[CpuFamily::Z80, CpuFamily::Ez80];

pub const EXACT_INSTRUCTIONS: &[InstructionSpec] = &[
    z80_ez80("nop", &[0x00]),
    z80_ez80("di", &[0xF3]),
    z80_ez80("ei", &[0xFB]),
    z80_ez80("halt", &[0x76]),
    z80_ez80("ret", &[0xC9]),
    z80_ez80("ret nz", &[0xC0]),
    z80_ez80("ret z", &[0xC8]),
    z80_ez80("ret nc", &[0xD0]),
    z80_ez80("ret c", &[0xD8]),
    z80_ez80("ret po", &[0xE0]),
    z80_ez80("ret pe", &[0xE8]),
    z80_ez80("ret p", &[0xF0]),
    z80_ez80("ret m", &[0xF8]),
    z80_ez80("reti", &[0xED, 0x4D]),
    z80_ez80("retn", &[0xED, 0x45]),
    z80_ez80("or a", &[0xB7]),
    z80_ez80("xor a", &[0xAF]),
    z80_ez80("scf", &[0x37]),
    z80_ez80("ccf", &[0x3F]),
    z80_ez80("cpl", &[0x2F]),
    z80_ez80("daa", &[0x27]),
    z80_ez80("neg", &[0xED, 0x44]),
    z80_ez80("rlca", &[0x07]),
    z80_ez80("rla", &[0x17]),
    z80_ez80("rrca", &[0x0F]),
    z80_ez80("rra", &[0x1F]),
    z80_ez80("rld", &[0xED, 0x6F]),
    z80_ez80("rrd", &[0xED, 0x67]),
    z80_ez80("ex de, hl", &[0xEB]),
    z80_ez80("ex af, af'", &[0x08]),
    z80_ez80("exx", &[0xD9]),
    z80_ez80("im 0", &[0xED, 0x46]),
    z80_ez80("im 1", &[0xED, 0x56]),
    z80_ez80("im 2", &[0xED, 0x5E]),
];

const fn z80_ez80(syntax: &'static str, bytes: &'static [u8]) -> InstructionSpec {
    InstructionSpec {
        syntax,
        cpus: Z80_AND_EZ80,
        bytes,
    }
}

pub fn exact_instruction(cpu: CpuFamily, text: &str) -> Option<&'static InstructionSpec> {
    EXACT_INSTRUCTIONS
        .iter()
        .find(|instruction| instruction.syntax == text && instruction.cpus.contains(&cpu))
}

pub fn instruction_set(cpu: CpuFamily) -> impl Iterator<Item = &'static InstructionSpec> {
    EXACT_INSTRUCTIONS
        .iter()
        .filter(move |instruction| instruction.cpus.contains(&cpu))
}

pub fn encode_generated_instruction(
    cpu: CpuFamily,
    text: &str,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    if !matches!(cpu, CpuFamily::Ez80 | CpuFamily::Z80) {
        return Ok(None);
    }
    if let Some(instruction) = exact_instruction(cpu, text) {
        return Ok(Some(instruction.bytes.to_vec()));
    }
    if let Some((dst, src)) = parse_ld_operands(text) {
        if let (Some(dst), Some(src)) = (reg8_code(dst), reg8_code(src)) {
            return Ok(Some(vec![0x40 + dst * 8 + src]));
        }
        if let Some(dst) = reg8_code(dst) {
            if reg8_code(src).is_none() && is_numeric_literal(src) {
                return Ok(Some(vec![ld_reg8_imm_opcode(dst), parse_u8(src)?]));
            }
        }
    }
    if let Some((inc, register)) = parse_inc_dec_reg8(text) {
        let base = if inc { 0x04 } else { 0x05 };
        return Ok(Some(vec![base + register * 8]));
    }
    if let Some((op, register)) = parse_accumulator_alu_reg8_or_hl(text) {
        return Ok(Some(vec![accumulator_alu_reg8_opcode(op, register)]));
    }
    Ok(None)
}

pub fn generated_instruction_len(cpu: CpuFamily, text: &str) -> Result<Option<usize>, Diagnostic> {
    Ok(encode_generated_instruction(cpu, text)?.map(|bytes| bytes.len()))
}

fn parse_ld_operands(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("ld ")?;
    let (dst, src) = rest.split_once(',')?;
    Some((dst.trim(), src.trim()))
}

fn parse_inc_dec_reg8(text: &str) -> Option<(bool, u8)> {
    if let Some(register) = text.strip_prefix("inc ") {
        return Some((true, reg8_code(register.trim())?));
    }
    if let Some(register) = text.strip_prefix("dec ") {
        return Some((false, reg8_code(register.trim())?));
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccumulatorAluOp {
    Add,
    Adc,
    Sub,
    Sbc,
    And,
    Or,
    Xor,
    Cp,
}

fn parse_accumulator_alu_reg8_or_hl(text: &str) -> Option<(AccumulatorAluOp, u8)> {
    if let Some(src) = text.strip_prefix("add a,") {
        return Some((AccumulatorAluOp::Add, reg8_or_hl_code(src.trim())?));
    }
    if let Some(src) = text.strip_prefix("adc a,") {
        return Some((AccumulatorAluOp::Adc, reg8_or_hl_code(src.trim())?));
    }
    if let Some(src) = text.strip_prefix("sbc a,") {
        return Some((AccumulatorAluOp::Sbc, reg8_or_hl_code(src.trim())?));
    }
    for (prefix, op) in [
        ("sub ", AccumulatorAluOp::Sub),
        ("and ", AccumulatorAluOp::And),
        ("or ", AccumulatorAluOp::Or),
        ("xor ", AccumulatorAluOp::Xor),
        ("cp ", AccumulatorAluOp::Cp),
    ] {
        if let Some(src) = text.strip_prefix(prefix) {
            return Some((op, reg8_or_hl_code(src.trim())?));
        }
    }
    None
}

fn reg8_code(register: &str) -> Option<u8> {
    match register {
        "b" => Some(0),
        "c" => Some(1),
        "d" => Some(2),
        "e" => Some(3),
        "h" => Some(4),
        "l" => Some(5),
        "a" => Some(7),
        _ => None,
    }
}

fn reg8_or_hl_code(register: &str) -> Option<u8> {
    if register == "(hl)" {
        return Some(6);
    }
    reg8_code(register)
}

fn ld_reg8_imm_opcode(register: u8) -> u8 {
    match register {
        0 => 0x06,
        1 => 0x0E,
        2 => 0x16,
        3 => 0x1E,
        4 => 0x26,
        5 => 0x2E,
        7 => 0x3E,
        _ => unreachable!("invalid 8-bit register code {register}"),
    }
}

fn accumulator_alu_reg8_opcode(op: AccumulatorAluOp, register: u8) -> u8 {
    let base = match op {
        AccumulatorAluOp::Add => 0x80,
        AccumulatorAluOp::Adc => 0x88,
        AccumulatorAluOp::Sub => 0x90,
        AccumulatorAluOp::Sbc => 0x98,
        AccumulatorAluOp::And => 0xA0,
        AccumulatorAluOp::Xor => 0xA8,
        AccumulatorAluOp::Or => 0xB0,
        AccumulatorAluOp::Cp => 0xB8,
    };
    base + register
}

fn parse_u8(text: &str) -> Result<u8, Diagnostic> {
    let value = parse_number(text)?;
    if value > 0xFF {
        return Err(Diagnostic::new(format!("value {text} is outside u8 range")));
    }
    Ok(value as u8)
}

fn is_numeric_literal(text: &str) -> bool {
    let text = text.trim().trim_end_matches(',');
    text.strip_prefix("0x")
        .is_some_and(|hex| !hex.is_empty() && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
        || text
            .strip_suffix('h')
            .is_some_and(|hex| !hex.is_empty() && hex.chars().all(|ch| ch.is_ascii_hexdigit()))
        || text.chars().all(|ch| ch.is_ascii_digit())
}

fn parse_number(text: &str) -> Result<u32, Diagnostic> {
    let text = text.trim().trim_end_matches(',');
    if let Some(hex) = text.strip_suffix('h') {
        u32::from_str_radix(hex, 16)
    } else if let Some(hex) = text.strip_prefix("0x") {
        u32::from_str_radix(hex, 16)
    } else {
        text.parse()
    }
    .map_err(|_| Diagnostic::new(format!("invalid numeric operand `{text}`")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_instruction_metadata_encodes_common_ops() {
        assert_eq!(
            exact_instruction(CpuFamily::Ez80, "nop").unwrap().bytes,
            &[0x00]
        );
        assert_eq!(
            exact_instruction(CpuFamily::Ez80, "reti").unwrap().bytes,
            &[0xED, 0x4D]
        );
    }

    #[test]
    fn metadata_can_generate_z80_subset() {
        let z80 = instruction_set(CpuFamily::Z80).collect::<Vec<_>>();
        assert!(z80.iter().any(|instruction| instruction.syntax == "ret"));
        assert!(z80.iter().any(|instruction| instruction.syntax == "im 2"));
    }

    #[test]
    fn generated_instruction_metadata_encodes_operand_families() {
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "ld b, a").unwrap(),
            Some(vec![0x47])
        );
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "ld a, 7Fh").unwrap(),
            Some(vec![0x3E, 0x7F])
        );
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "inc c").unwrap(),
            Some(vec![0x0C])
        );
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "add a, c").unwrap(),
            Some(vec![0x81])
        );
    }
}
