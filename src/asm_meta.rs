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
    z80_ez80("ex (sp), hl", &[0xE3]),
    z80_ez80("ex (sp), ix", &[0xDD, 0xE3]),
    z80_ez80("ex (sp), iy", &[0xFD, 0xE3]),
    z80_ez80("exx", &[0xD9]),
    z80_ez80("ld sp, hl", &[0xF9]),
    z80_ez80("ld sp, ix", &[0xDD, 0xF9]),
    z80_ez80("ld sp, iy", &[0xFD, 0xF9]),
    z80_ez80("jp (hl)", &[0xE9]),
    z80_ez80("jp (ix)", &[0xDD, 0xE9]),
    z80_ez80("jp (iy)", &[0xFD, 0xE9]),
    z80_ez80("push af", &[0xF5]),
    z80_ez80("push bc", &[0xC5]),
    z80_ez80("push de", &[0xD5]),
    z80_ez80("push hl", &[0xE5]),
    z80_ez80("push ix", &[0xDD, 0xE5]),
    z80_ez80("push iy", &[0xFD, 0xE5]),
    z80_ez80("pop af", &[0xF1]),
    z80_ez80("pop bc", &[0xC1]),
    z80_ez80("pop de", &[0xD1]),
    z80_ez80("pop hl", &[0xE1]),
    z80_ez80("pop ix", &[0xDD, 0xE1]),
    z80_ez80("pop iy", &[0xFD, 0xE1]),
    z80_ez80("ld i, a", &[0xED, 0x47]),
    z80_ez80("ld r, a", &[0xED, 0x4F]),
    z80_ez80("ld a, i", &[0xED, 0x57]),
    z80_ez80("ld a, r", &[0xED, 0x5F]),
    z80_ez80("im 0", &[0xED, 0x46]),
    z80_ez80("im 1", &[0xED, 0x56]),
    z80_ez80("im 2", &[0xED, 0x5E]),
    z80_ez80("ldi", &[0xED, 0xA0]),
    z80_ez80("ldir", &[0xED, 0xB0]),
    z80_ez80("ldd", &[0xED, 0xA8]),
    z80_ez80("lddr", &[0xED, 0xB8]),
    z80_ez80("cpi", &[0xED, 0xA1]),
    z80_ez80("cpir", &[0xED, 0xB1]),
    z80_ez80("cpd", &[0xED, 0xA9]),
    z80_ez80("cpdr", &[0xED, 0xB9]),
    z80_ez80("ini", &[0xED, 0xA2]),
    z80_ez80("inir", &[0xED, 0xB2]),
    z80_ez80("ind", &[0xED, 0xAA]),
    z80_ez80("indr", &[0xED, 0xBA]),
    z80_ez80("outi", &[0xED, 0xA3]),
    z80_ez80("otir", &[0xED, 0xB3]),
    z80_ez80("outd", &[0xED, 0xAB]),
    z80_ez80("otdr", &[0xED, 0xBB]),
    z80_ez80("mlt bc", &[0xED, 0x4C]),
    z80_ez80("mlt de", &[0xED, 0x5C]),
    z80_ez80("mlt hl", &[0xED, 0x6C]),
    z80_ez80("mlt sp", &[0xED, 0x7C]),
    z80_ez80("adc hl, bc", &[0xED, 0x4A]),
    z80_ez80("adc hl, de", &[0xED, 0x5A]),
    z80_ez80("adc hl, hl", &[0xED, 0x6A]),
    z80_ez80("adc hl, sp", &[0xED, 0x7A]),
    z80_ez80("sbc hl, bc", &[0xED, 0x42]),
    z80_ez80("sbc hl, de", &[0xED, 0x52]),
    z80_ez80("sbc hl, hl", &[0xED, 0x62]),
    z80_ez80("sbc hl, sp", &[0xED, 0x72]),
    z80_ez80("inc ix", &[0xDD, 0x23]),
    z80_ez80("inc iy", &[0xFD, 0x23]),
    z80_ez80("dec ix", &[0xDD, 0x2B]),
    z80_ez80("dec iy", &[0xFD, 0x2B]),
    z80_ez80("add ix, bc", &[0xDD, 0x09]),
    z80_ez80("add ix, de", &[0xDD, 0x19]),
    z80_ez80("add ix, ix", &[0xDD, 0x29]),
    z80_ez80("add ix, sp", &[0xDD, 0x39]),
    z80_ez80("add iy, bc", &[0xFD, 0x09]),
    z80_ez80("add iy, de", &[0xFD, 0x19]),
    z80_ez80("add iy, iy", &[0xFD, 0x29]),
    z80_ez80("add iy, sp", &[0xFD, 0x39]),
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
    if let Some((inc, register)) = parse_inc_dec_reg16(text) {
        let base = if inc { 0x03 } else { 0x0B };
        return Ok(Some(vec![base + register * 0x10]));
    }
    if let Some(register) = parse_add_hl_reg16(text) {
        return Ok(Some(vec![0x09 + register * 0x10]));
    }
    if let Some((op, register)) = parse_accumulator_alu_reg8_or_hl(text) {
        return Ok(Some(vec![accumulator_alu_reg8_opcode(op, register)]));
    }
    if let Some((op, value)) = parse_accumulator_alu_imm(text)? {
        return Ok(Some(vec![accumulator_alu_imm_opcode(op), value]));
    }
    if let Some(opcode) = parse_bit_operation_reg8_or_hl(text)? {
        return Ok(Some(vec![0xCB, opcode]));
    }
    if let Some(opcode) = parse_cb_reg8_or_hl_operation(text)? {
        return Ok(Some(vec![0xCB, opcode]));
    }
    if let Some(bytes) = parse_io_instruction(text)? {
        return Ok(Some(bytes));
    }
    if let Some(bytes) = parse_rst_instruction(text)? {
        return Ok(Some(bytes));
    }
    Ok(None)
}

pub fn generated_instruction_len(cpu: CpuFamily, text: &str) -> Result<Option<usize>, Diagnostic> {
    if let Some(branch) = branch_instruction(cpu, text) {
        return Ok(Some(branch.len()));
    }
    if let Some(load) = imm24_load_instruction(cpu, text) {
        return Ok(Some(load.len()));
    }
    Ok(encode_generated_instruction(cpu, text)?.map(|bytes| bytes.len()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BranchInstruction<'a> {
    pub opcode: u8,
    pub target: &'a str,
    pub width: BranchWidth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BranchWidth {
    Relative8,
    Absolute24,
}

impl BranchInstruction<'_> {
    pub const fn len(self) -> usize {
        match self.width {
            BranchWidth::Relative8 => 2,
            BranchWidth::Absolute24 => 4,
        }
    }
}

pub fn branch_instruction<'a>(cpu: CpuFamily, text: &'a str) -> Option<BranchInstruction<'a>> {
    if !matches!(cpu, CpuFamily::Ez80 | CpuFamily::Z80) {
        return None;
    }
    for (prefix, opcode) in ABSOLUTE_BRANCH_FORMS {
        if let Some(target) = text.strip_prefix(prefix) {
            return Some(BranchInstruction {
                opcode: *opcode,
                target: target.trim(),
                width: BranchWidth::Absolute24,
            });
        }
    }
    for (prefix, opcode) in RELATIVE_BRANCH_FORMS {
        if let Some(target) = text.strip_prefix(prefix) {
            return Some(BranchInstruction {
                opcode: *opcode,
                target: target.trim(),
                width: BranchWidth::Relative8,
            });
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Imm24LoadInstruction<'a> {
    pub prefix: &'static [u8],
    pub value: &'a str,
}

impl Imm24LoadInstruction<'_> {
    pub const fn len(self) -> usize {
        self.prefix.len() + 3
    }
}

pub fn imm24_load_instruction<'a>(
    cpu: CpuFamily,
    text: &'a str,
) -> Option<Imm24LoadInstruction<'a>> {
    if !matches!(cpu, CpuFamily::Ez80) {
        return None;
    }
    for (prefix, bytes) in IMM24_LOAD_FORMS {
        if let Some(value) = text.strip_prefix(prefix) {
            let value = value.trim();
            if value.starts_with('(') {
                return None;
            }
            return Some(Imm24LoadInstruction {
                prefix: bytes,
                value,
            });
        }
    }
    None
}

const ABSOLUTE_BRANCH_FORMS: &[(&str, u8)] = &[
    ("call nz,", 0xC4),
    ("call z,", 0xCC),
    ("call nc,", 0xD4),
    ("call c,", 0xDC),
    ("call po,", 0xE4),
    ("call pe,", 0xEC),
    ("call p,", 0xF4),
    ("call m,", 0xFC),
    ("call ", 0xCD),
    ("jp z,", 0xCA),
    ("jp nz,", 0xC2),
    ("jp c,", 0xDA),
    ("jp nc,", 0xD2),
    ("jp po,", 0xE2),
    ("jp pe,", 0xEA),
    ("jp p,", 0xF2),
    ("jp m,", 0xFA),
    ("jp ", 0xC3),
];

const RELATIVE_BRANCH_FORMS: &[(&str, u8)] = &[
    ("jr z,", 0x28),
    ("jr nz,", 0x20),
    ("jr c,", 0x38),
    ("jr nc,", 0x30),
    ("jr ", 0x18),
    ("djnz ", 0x10),
];

const IMM24_LOAD_FORMS: &[(&str, &[u8])] = &[
    ("ld bc,", &[0x01]),
    ("ld de,", &[0x11]),
    ("ld hl,", &[0x21]),
    ("ld sp,", &[0x31]),
    ("ld ix,", &[0xDD, 0x21]),
    ("ld iy,", &[0xFD, 0x21]),
];

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

fn parse_inc_dec_reg16(text: &str) -> Option<(bool, u8)> {
    if let Some(register) = text.strip_prefix("inc ") {
        return Some((true, reg16_code(register.trim())?));
    }
    if let Some(register) = text.strip_prefix("dec ") {
        return Some((false, reg16_code(register.trim())?));
    }
    None
}

fn parse_add_hl_reg16(text: &str) -> Option<u8> {
    let register = text.strip_prefix("add hl,")?.trim();
    reg16_code(register)
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

fn parse_accumulator_alu_imm(text: &str) -> Result<Option<(AccumulatorAluOp, u8)>, Diagnostic> {
    if let Some(src) = text.strip_prefix("add a,") {
        return parse_alu_imm(src, AccumulatorAluOp::Add);
    }
    if let Some(src) = text.strip_prefix("adc a,") {
        return parse_alu_imm(src, AccumulatorAluOp::Adc);
    }
    if let Some(src) = text.strip_prefix("sbc a,") {
        return parse_alu_imm(src, AccumulatorAluOp::Sbc);
    }
    for (prefix, op) in [
        ("sub ", AccumulatorAluOp::Sub),
        ("and ", AccumulatorAluOp::And),
        ("or ", AccumulatorAluOp::Or),
        ("xor ", AccumulatorAluOp::Xor),
        ("cp ", AccumulatorAluOp::Cp),
    ] {
        if let Some(src) = text.strip_prefix(prefix) {
            return parse_alu_imm(src, op);
        }
    }
    Ok(None)
}

fn parse_alu_imm(
    src: &str,
    op: AccumulatorAluOp,
) -> Result<Option<(AccumulatorAluOp, u8)>, Diagnostic> {
    let src = src.trim();
    if reg8_or_hl_code(src).is_some() || !is_numeric_literal(src) {
        return Ok(None);
    }
    Ok(Some((op, parse_u8(src)?)))
}

fn parse_bit_operation_reg8_or_hl(text: &str) -> Result<Option<u8>, Diagnostic> {
    for (prefix, base) in [("bit ", 0x40), ("res ", 0x80), ("set ", 0xC0)] {
        let Some(rest) = text.strip_prefix(prefix) else {
            continue;
        };
        let Some((bit, register)) = rest.split_once(',') else {
            return Err(Diagnostic::new(format!("invalid bit operation `{text}`")));
        };
        let bit = parse_u8(bit.trim())?;
        if bit > 7 {
            return Err(Diagnostic::new(format!("bit index {bit} is outside 0..7")));
        }
        let register_text = register.trim();
        if is_indexed_indirect(register_text) {
            return Ok(None);
        }
        let Some(register) = reg8_or_hl_code(register_text) else {
            return Err(Diagnostic::new(format!(
                "invalid bit register `{}`",
                register_text
            )));
        };
        return Ok(Some(base + bit * 8 + register));
    }
    Ok(None)
}

fn parse_cb_reg8_or_hl_operation(text: &str) -> Result<Option<u8>, Diagnostic> {
    let Some((base, register)) = parse_cb_operation_operand(text) else {
        return Ok(None);
    };
    let register_text = register.trim();
    if is_indexed_indirect(register_text) {
        return Ok(None);
    }
    let Some(register) = reg8_or_hl_code(register_text) else {
        return Err(Diagnostic::new(format!(
            "invalid rotate/shift register `{}`",
            register_text
        )));
    };
    Ok(Some(base + register))
}

fn parse_cb_operation_operand(text: &str) -> Option<(u8, &str)> {
    if let Some(register) = text.strip_prefix("rlc ") {
        Some((0x00, register))
    } else if let Some(register) = text.strip_prefix("rrc ") {
        Some((0x08, register))
    } else if let Some(register) = text.strip_prefix("rl ") {
        Some((0x10, register))
    } else if let Some(register) = text.strip_prefix("rr ") {
        Some((0x18, register))
    } else if let Some(register) = text.strip_prefix("sla ") {
        Some((0x20, register))
    } else if let Some(register) = text.strip_prefix("sra ") {
        Some((0x28, register))
    } else if let Some(register) = text.strip_prefix("srl ") {
        Some((0x38, register))
    } else {
        None
    }
}

fn parse_io_instruction(text: &str) -> Result<Option<Vec<u8>>, Diagnostic> {
    if let Some(port) = text
        .strip_prefix("in a, (")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        if port.trim() == "c" {
            return Ok(Some(vec![0xED, 0x78]));
        }
        return Ok(Some(vec![0xDB, parse_u8(port.trim())?]));
    }
    if let Some(rest) = text.strip_prefix("in ") {
        let Some((register, port)) = rest.split_once(',') else {
            return Err(Diagnostic::new(format!("invalid in syntax `{text}`")));
        };
        let Some(register) = reg8_code(register.trim()) else {
            return Ok(None);
        };
        if port.trim() != "(c)" {
            return Ok(None);
        }
        return Ok(Some(vec![0xED, 0x40 + register * 8]));
    }
    if let Some(rest) = text.strip_prefix("out ") {
        let Some((port, register)) = rest.split_once(',') else {
            return Err(Diagnostic::new(format!("invalid out syntax `{text}`")));
        };
        if port.trim() == "(c)" {
            let Some(register) = reg8_code(register.trim()) else {
                return Ok(None);
            };
            return Ok(Some(vec![0xED, 0x41 + register * 8]));
        }
        let port = port
            .trim()
            .strip_prefix('(')
            .and_then(|port| port.strip_suffix(')'))
            .ok_or_else(|| Diagnostic::new(format!("invalid out port syntax `{text}`")))?;
        if register.trim() != "a" {
            return Ok(None);
        }
        return Ok(Some(vec![0xD3, parse_u8(port)?]));
    }
    if let Some(rest) = text.strip_prefix("in0 ") {
        let port = rest
            .trim()
            .strip_prefix("a, (")
            .and_then(|rest| rest.strip_suffix(')'))
            .ok_or_else(|| Diagnostic::new(format!("invalid in0 syntax `{text}`")))?;
        return Ok(Some(vec![0xED, 0x38, parse_u8(port)?]));
    }
    if let Some(rest) = text.strip_prefix("out0 ") {
        let port = rest
            .trim()
            .strip_prefix('(')
            .and_then(|rest| rest.split_once(')'))
            .ok_or_else(|| Diagnostic::new(format!("invalid out0 syntax `{text}`")))?
            .0;
        return Ok(Some(vec![0xED, 0x39, parse_u8(port)?]));
    }
    Ok(None)
}

fn parse_rst_instruction(text: &str) -> Result<Option<Vec<u8>>, Diagnostic> {
    let (lis, target) = if let Some(target) = text.strip_prefix("rst.lis ") {
        (true, target)
    } else if let Some(target) = text.strip_prefix("rst ") {
        (false, target)
    } else {
        return Ok(None);
    };
    let target = parse_number(target.trim())?;
    if target > 0x38 || target % 8 != 0 {
        return Err(Diagnostic::new(format!(
            "restart target 0x{target:X} is not one of 0x00, 0x08, ..., 0x38"
        )));
    }
    let opcode = 0xC7 + target as u8;
    if lis {
        Ok(Some(vec![0x49, opcode]))
    } else {
        Ok(Some(vec![opcode]))
    }
}

fn is_indexed_indirect(text: &str) -> bool {
    text.starts_with("(ix") || text.starts_with("(iy")
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

fn reg16_code(register: &str) -> Option<u8> {
    match register {
        "bc" => Some(0),
        "de" => Some(1),
        "hl" => Some(2),
        "sp" => Some(3),
        _ => None,
    }
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

fn accumulator_alu_imm_opcode(op: AccumulatorAluOp) -> u8 {
    match op {
        AccumulatorAluOp::Add => 0xC6,
        AccumulatorAluOp::Adc => 0xCE,
        AccumulatorAluOp::Sub => 0xD6,
        AccumulatorAluOp::Sbc => 0xDE,
        AccumulatorAluOp::And => 0xE6,
        AccumulatorAluOp::Xor => 0xEE,
        AccumulatorAluOp::Or => 0xF6,
        AccumulatorAluOp::Cp => 0xFE,
    }
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
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "inc hl").unwrap(),
            Some(vec![0x23])
        );
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "add hl, de").unwrap(),
            Some(vec![0x19])
        );
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "srl a").unwrap(),
            Some(vec![0xCB, 0x3F])
        );
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "bit 3, (hl)").unwrap(),
            Some(vec![0xCB, 0x5E])
        );
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "in a, (34h)").unwrap(),
            Some(vec![0xDB, 0x34])
        );
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "out0 (0Ch), a").unwrap(),
            Some(vec![0xED, 0x39, 0x0C])
        );
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "rst.lis 10h").unwrap(),
            Some(vec![0x49, 0xD7])
        );
        assert_eq!(
            encode_generated_instruction(CpuFamily::Ez80, "xor 55h").unwrap(),
            Some(vec![0xEE, 0x55])
        );
    }

    #[test]
    fn branch_metadata_describes_control_flow_widths() {
        let call = branch_instruction(CpuFamily::Ez80, "call nz, _main").unwrap();
        assert_eq!(call.opcode, 0xC4);
        assert_eq!(call.target, "_main");
        assert_eq!(call.len(), 4);

        let jr = branch_instruction(CpuFamily::Ez80, "jr z, .done").unwrap();
        assert_eq!(jr.opcode, 0x28);
        assert_eq!(jr.target, ".done");
        assert_eq!(jr.len(), 2);
    }
}
