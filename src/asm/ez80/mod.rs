use crate::diagnostic::Diagnostic;
use crate::target::AssemblerCpu;

pub mod emitter;

pub use emitter::{
    AssemblyOptions, CheckedEz80Program, emit_ez80_assembly, emit_ez80_assembly_from_checked,
    emit_ez80_assembly_with_debug_comments, emit_ez80_assembly_with_options,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstructionSpec {
    pub syntax: &'static str,
    pub cpus: &'static [AssemblerCpu],
    pub bytes: &'static [u8],
}

const Z80_PLUS: &[AssemblerCpu] = &[
    AssemblerCpu::Z80,
    AssemblerCpu::Z80N,
    AssemblerCpu::Z180,
    AssemblerCpu::Ez80,
];
const Z80N_ONLY: &[AssemblerCpu] = &[AssemblerCpu::Z80N];
const Z180_ONLY: &[AssemblerCpu] = &[AssemblerCpu::Z180];
const Z180_PLUS: &[AssemblerCpu] = &[AssemblerCpu::Z180, AssemblerCpu::Ez80];

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
    z80_ez80("inc sp", &[0x33]),
    z80_ez80("dec sp", &[0x3B]),
    z80_ez80("inc (hl)", &[0x34]),
    z80_ez80("dec (hl)", &[0x35]),
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
    z80_ez80("ld a, (bc)", &[0x0A]),
    z80_ez80("ld a, (de)", &[0x1A]),
    z80_ez80("ld a, (hl)", &[0x7E]),
    z80_ez80("ld (bc), a", &[0x02]),
    z80_ez80("ld (de), a", &[0x12]),
    z80_ez80("ld (hl), a", &[0x77]),
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
    z80n("swapnib", &[0xED, 0x23]),
    z80n("mirror a", &[0xED, 0x24]),
    z80n("bsla de, b", &[0xED, 0x28]),
    z80n("bsra de, b", &[0xED, 0x29]),
    z80n("bsrl de, b", &[0xED, 0x2A]),
    z80n("bsrf de, b", &[0xED, 0x2B]),
    z80n("brlc de, b", &[0xED, 0x2C]),
    z80n("mul d, e", &[0xED, 0x30]),
    z80n("add hl, a", &[0xED, 0x31]),
    z80n("add de, a", &[0xED, 0x32]),
    z80n("add bc, a", &[0xED, 0x33]),
    z80n("outinb", &[0xED, 0x90]),
    z80n("pixeldn", &[0xED, 0x93]),
    z80n("pixelad", &[0xED, 0x94]),
    z80n("setae", &[0xED, 0x95]),
    z80n("jp (c)", &[0xED, 0x98]),
    z80n("ldix", &[0xED, 0xA4]),
    z80n("ldws", &[0xED, 0xA5]),
    z80n("lddx", &[0xED, 0xAC]),
    z80n("ldirx", &[0xED, 0xB4]),
    z80n("ldpirx", &[0xED, 0xB7]),
    z80n("lddrx", &[0xED, 0xBC]),
    z180("otim", &[0xED, 0x83]),
    z180("otimr", &[0xED, 0x93]),
    z180("otdm", &[0xED, 0x8B]),
    z180("otdmr", &[0xED, 0x9B]),
    z180_ez80("mlt bc", &[0xED, 0x4C]),
    z180_ez80("mlt de", &[0xED, 0x5C]),
    z180_ez80("mlt hl", &[0xED, 0x6C]),
    z180_ez80("mlt sp", &[0xED, 0x7C]),
    z180_ez80("slp", &[0xED, 0x76]),
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
        cpus: Z80_PLUS,
        bytes,
    }
}

const fn z80n(syntax: &'static str, bytes: &'static [u8]) -> InstructionSpec {
    InstructionSpec {
        syntax,
        cpus: Z80N_ONLY,
        bytes,
    }
}

const fn z180(syntax: &'static str, bytes: &'static [u8]) -> InstructionSpec {
    InstructionSpec {
        syntax,
        cpus: Z180_ONLY,
        bytes,
    }
}

const fn z180_ez80(syntax: &'static str, bytes: &'static [u8]) -> InstructionSpec {
    InstructionSpec {
        syntax,
        cpus: Z180_PLUS,
        bytes,
    }
}

pub fn exact_instruction(cpu: AssemblerCpu, text: &str) -> Option<&'static InstructionSpec> {
    EXACT_INSTRUCTIONS.iter().find(|instruction| {
        exact_syntax_matches(instruction.syntax, text) && instruction.cpus.contains(&cpu)
    })
}

fn exact_syntax_matches(expected: &str, actual: &str) -> bool {
    expected == actual || expected.replace(", ", ",") == actual
}

pub fn instruction_set(cpu: AssemblerCpu) -> impl Iterator<Item = &'static InstructionSpec> {
    EXACT_INSTRUCTIONS
        .iter()
        .filter(move |instruction| instruction.cpus.contains(&cpu))
}

pub fn encode_generated_instruction(
    cpu: AssemblerCpu,
    text: &str,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    if matches!(cpu, AssemblerCpu::I8080 | AssemblerCpu::I8085) {
        return encode_intel_8080_instruction(cpu, text);
    }
    if !cpu.supports_z80_syntax() {
        return Ok(None);
    }
    if let Some((prefix, base)) = ez80_mode_suffixed_instruction(cpu, text) {
        if let Some(mut bytes) = encode_generated_instruction(cpu, &base)? {
            bytes.insert(0, prefix);
            return Ok(Some(bytes));
        }
    }
    if let Some(instruction) = exact_instruction(cpu, text) {
        return Ok(Some(instruction.bytes.to_vec()));
    }
    if let Some(bytes) = parse_z80n_instruction(cpu, text)? {
        return Ok(Some(bytes));
    }
    if let Some(bytes) = parse_z180_instruction(cpu, text)? {
        return Ok(Some(bytes));
    }
    if let Some(bytes) = parse_prefixed_reg8_instruction(text)? {
        return Ok(Some(bytes));
    }
    if let Some((dst, src)) = parse_ld_operands(text) {
        if let Some(bytes) = encode_ld_reg16_imm(cpu, dst, src)? {
            return Ok(Some(bytes));
        }
        if let Some(bytes) = encode_ld_direct16(cpu, dst, src)? {
            return Ok(Some(bytes));
        }
        if let (Some(dst), Some(src)) = (reg8_code(dst), reg8_code(src)) {
            return Ok(Some(vec![0x40 + dst * 8 + src]));
        }
        if let (Some(dst), Some(src)) = (reg8_code(dst), parse_hl_indirect(src)) {
            return Ok(Some(vec![0x40 + dst * 8 + src]));
        }
        if let (Some(dst), Some(src)) = (parse_hl_indirect(dst), reg8_code(src)) {
            return Ok(Some(vec![0x40 + dst * 8 + src]));
        }
        if let Some(dst) = parse_hl_indirect(dst) {
            if reg8_code(src).is_none() && !src.starts_with('(') {
                return Ok(Some(vec![0x06 + dst * 8, parse_u8(src)?]));
            }
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
    if let Some(bytes) = parse_index_instruction(text)? {
        return Ok(Some(bytes));
    }
    if let Some(bytes) = parse_index_cb_instruction(text)? {
        return Ok(Some(bytes));
    }
    if let Some(opcode) = parse_bit_operation_reg8_or_hl(text)? {
        return Ok(Some(vec![0xCB, opcode]));
    }
    if let Some(opcode) = parse_cb_reg8_or_hl_operation(text)? {
        return Ok(Some(vec![0xCB, opcode]));
    }
    if let Some(bytes) = parse_lea_instruction(cpu, text)? {
        return Ok(Some(bytes));
    }
    if let Some(bytes) = parse_io_instruction(cpu, text)? {
        return Ok(Some(bytes));
    }
    if let Some(bytes) = parse_rst_instruction(text)? {
        return Ok(Some(bytes));
    }
    Ok(None)
}

fn encode_ld_direct16(
    cpu: AssemblerCpu,
    dst: &str,
    src: &str,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    if !matches!(
        cpu,
        AssemblerCpu::Z80 | AssemblerCpu::Z80N | AssemblerCpu::Z180
    ) {
        return Ok(None);
    }
    if let Some(addr) = parse_wrapped_indirect(dst) {
        let Some(prefix) = direct16_store_prefix(src) else {
            return Ok(None);
        };
        let value = parse_u16(addr)?;
        let mut bytes = prefix.to_vec();
        bytes.push(value as u8);
        bytes.push((value >> 8) as u8);
        return Ok(Some(bytes));
    }
    if let Some(addr) = parse_wrapped_indirect(src) {
        let Some(prefix) = direct16_load_prefix(dst) else {
            return Ok(None);
        };
        let value = parse_u16(addr)?;
        let mut bytes = prefix.to_vec();
        bytes.push(value as u8);
        bytes.push((value >> 8) as u8);
        return Ok(Some(bytes));
    }
    Ok(None)
}

fn parse_z80n_instruction(cpu: AssemblerCpu, text: &str) -> Result<Option<Vec<u8>>, Diagnostic> {
    if cpu != AssemblerCpu::Z80N {
        return Ok(None);
    }
    if let Some(value) = text.strip_prefix("test ") {
        return Ok(Some(vec![0xED, 0x27, parse_u8(value)?]));
    }
    if let Some(value) = text.strip_prefix("push ") {
        if !is_numeric_literal(value) {
            return Ok(None);
        }
        let value = parse_u16(value)?;
        return Ok(Some(vec![0xED, 0x8A, (value >> 8) as u8, value as u8]));
    }
    if let Some(value) = text.strip_prefix("add hl,") {
        return parse_z80n_add_imm(value.trim(), 0x34);
    }
    if let Some(value) = text.strip_prefix("add de,") {
        return parse_z80n_add_imm(value.trim(), 0x35);
    }
    if let Some(value) = text.strip_prefix("add bc,") {
        return parse_z80n_add_imm(value.trim(), 0x36);
    }
    if let Some(rest) = text.strip_prefix("nextreg ") {
        let Some((register, value)) = rest.split_once(',') else {
            return Err(Diagnostic::new(format!("invalid nextreg syntax `{text}`")));
        };
        let register = parse_u8(register.trim())?;
        let value = value.trim();
        if value == "a" {
            return Ok(Some(vec![0xED, 0x92, register]));
        }
        return Ok(Some(vec![0xED, 0x91, register, parse_u8(value)?]));
    }
    Ok(None)
}

fn parse_z80n_add_imm(value: &str, opcode: u8) -> Result<Option<Vec<u8>>, Diagnostic> {
    if !is_numeric_literal(value) {
        return Ok(None);
    }
    let value = parse_u16(value)?;
    Ok(Some(vec![0xED, opcode, value as u8, (value >> 8) as u8]))
}

fn parse_z180_instruction(cpu: AssemblerCpu, text: &str) -> Result<Option<Vec<u8>>, Diagnostic> {
    if cpu != AssemblerCpu::Z180 {
        return Ok(None);
    }
    if let Some(value) = text.strip_prefix("tstio ") {
        return Ok(Some(vec![0xED, 0x74, parse_u8(value)?]));
    }
    if let Some(value) = text.strip_prefix("tst ") {
        if is_numeric_literal(value) {
            return Ok(Some(vec![0xED, 0x64, parse_u8(value)?]));
        }
        let register = if value == "(hl)" {
            Some(6)
        } else {
            reg8_code(value)
        };
        if let Some(register) = register {
            return Ok(Some(vec![0xED, 0x04 + register * 8]));
        }
    }
    Ok(None)
}

fn direct16_load_prefix(register: &str) -> Option<&'static [u8]> {
    match register {
        "a" => Some(&[0x3A]),
        "hl" => Some(&[0x2A]),
        "bc" => Some(&[0xED, 0x4B]),
        "de" => Some(&[0xED, 0x5B]),
        "sp" => Some(&[0xED, 0x7B]),
        _ => None,
    }
}

fn direct16_store_prefix(register: &str) -> Option<&'static [u8]> {
    match register {
        "a" => Some(&[0x32]),
        "hl" => Some(&[0x22]),
        "bc" => Some(&[0xED, 0x43]),
        "de" => Some(&[0xED, 0x53]),
        "sp" => Some(&[0xED, 0x73]),
        _ => None,
    }
}

fn encode_ld_reg16_imm(
    cpu: AssemblerCpu,
    dst: &str,
    src: &str,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    if !matches!(
        cpu,
        AssemblerCpu::Z80 | AssemblerCpu::Z80N | AssemblerCpu::Z180
    ) {
        return Ok(None);
    }
    if !is_numeric_literal(src) {
        return Ok(None);
    }
    let prefix_and_opcode: &[u8] = match dst {
        "bc" => &[0x01],
        "de" => &[0x11],
        "hl" => &[0x21],
        "sp" => &[0x31],
        "ix" => &[0xDD, 0x21],
        "iy" => &[0xFD, 0x21],
        _ => return Ok(None),
    };
    let value = parse_u16(src)?;
    let mut bytes = prefix_and_opcode.to_vec();
    bytes.push(value as u8);
    bytes.push((value >> 8) as u8);
    Ok(Some(bytes))
}

pub fn generated_instruction_len(
    cpu: AssemblerCpu,
    text: &str,
) -> Result<Option<usize>, Diagnostic> {
    if matches!(cpu, AssemblerCpu::I8080 | AssemblerCpu::I8085) {
        if let Some(branch) = branch_instruction(cpu, text) {
            return Ok(Some(branch.len()));
        }
        return Ok(encode_generated_instruction(cpu, text)?.map(|bytes| bytes.len()));
    }
    if let Some((_prefix, base)) = ez80_mode_suffixed_instruction(cpu, text) {
        if let Some(len) = generated_instruction_len(cpu, &base)? {
            return Ok(Some(len + 1));
        }
    }
    if let Some(branch) = branch_instruction(cpu, text) {
        return Ok(Some(branch.len()));
    }
    if let Some(direct) = direct24_instruction(cpu, text) {
        return Ok(Some(direct.len()));
    }
    if let Some(load) = imm24_load_instruction(cpu, text) {
        return Ok(Some(load.len()));
    }
    Ok(encode_generated_instruction(cpu, text)?.map(|bytes| bytes.len()))
}

pub fn ez80_mode_suffixed_instruction(cpu: AssemblerCpu, text: &str) -> Option<(u8, String)> {
    if !cpu.supports_ez80_syntax() {
        return None;
    }
    let (mnemonic, rest) = text
        .split_once(char::is_whitespace)
        .map_or((text, ""), |(mnemonic, rest)| (mnemonic, rest.trim_start()));
    let (mnemonic, suffix) = mnemonic.rsplit_once('.')?;
    let prefix = match suffix {
        "sis" => 0x40,
        "lis" => 0x49,
        "sil" => 0x52,
        "lil" => 0x5B,
        _ => return None,
    };
    let base = if rest.is_empty() {
        mnemonic.to_owned()
    } else {
        format!("{mnemonic} {rest}")
    };
    Some((prefix, base))
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
    Absolute16,
    Absolute24,
}

impl BranchInstruction<'_> {
    pub const fn len(self) -> usize {
        match self.width {
            BranchWidth::Relative8 => 2,
            BranchWidth::Absolute16 => 3,
            BranchWidth::Absolute24 => 4,
        }
    }
}

pub fn branch_instruction<'a>(cpu: AssemblerCpu, text: &'a str) -> Option<BranchInstruction<'a>> {
    if matches!(cpu, AssemblerCpu::I8080 | AssemblerCpu::I8085) {
        return intel_8080_branch_instruction(text);
    }
    if !cpu.supports_z80_syntax() {
        return None;
    }
    for (prefix, opcode) in ABSOLUTE_BRANCH_FORMS {
        if let Some(target) = text.strip_prefix(prefix) {
            return Some(BranchInstruction {
                opcode: *opcode,
                target: target.trim(),
                width: if cpu == AssemblerCpu::Ez80 {
                    BranchWidth::Absolute24
                } else {
                    BranchWidth::Absolute16
                },
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
    cpu: AssemblerCpu,
    text: &'a str,
) -> Option<Imm24LoadInstruction<'a>> {
    if !cpu.supports_ez80_syntax() {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Direct24Instruction<'a> {
    pub prefix: &'static [u8],
    pub addr: &'a str,
}

impl Direct24Instruction<'_> {
    pub const fn len(self) -> usize {
        self.prefix.len() + 3
    }
}

pub fn direct24_instruction<'a>(
    cpu: AssemblerCpu,
    text: &'a str,
) -> Option<Direct24Instruction<'a>> {
    if !cpu.supports_ez80_syntax() {
        return None;
    }
    let (dst, src) = parse_ld_operands(text)?;
    for (register, bytes) in DIRECT24_LOAD_FORMS {
        if dst == *register {
            let addr = parse_wrapped_indirect(src)?;
            if is_register_indirect_addr(addr) {
                return None;
            }
            return Some(Direct24Instruction {
                prefix: bytes,
                addr,
            });
        }
    }
    let addr = parse_wrapped_indirect(dst)?;
    if is_register_indirect_addr(addr) {
        return None;
    }
    for (register, bytes) in DIRECT24_STORE_FORMS {
        if src == *register {
            return Some(Direct24Instruction {
                prefix: bytes,
                addr,
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

const DIRECT24_LOAD_FORMS: &[(&str, &[u8])] = &[
    ("a", &[0x3A]),
    ("hl", &[0x2A]),
    ("bc", &[0xED, 0x4B]),
    ("de", &[0xED, 0x5B]),
    ("sp", &[0xED, 0x7B]),
    ("ix", &[0xDD, 0x2A]),
    ("iy", &[0xFD, 0x2A]),
];

const DIRECT24_STORE_FORMS: &[(&str, &[u8])] = &[
    ("a", &[0x32]),
    ("hl", &[0x22]),
    ("bc", &[0xED, 0x43]),
    ("de", &[0xED, 0x53]),
    ("sp", &[0xED, 0x73]),
    ("ix", &[0xDD, 0x22]),
    ("iy", &[0xFD, 0x22]),
];

fn parse_ld_operands(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("ld ")?;
    let (dst, src) = rest.split_once(',')?;
    Some((dst.trim(), src.trim()))
}

fn parse_wrapped_indirect(text: &str) -> Option<&str> {
    text.strip_prefix('(')?.strip_suffix(')')
}

fn parse_hl_indirect(text: &str) -> Option<u8> {
    (text == "(hl)").then_some(6)
}

fn is_register_indirect_addr(addr: &str) -> bool {
    matches!(addr, "bc" | "de" | "hl")
        || is_index_indirect_addr(addr, "ix")
        || is_index_indirect_addr(addr, "iy")
}

fn is_index_indirect_addr(addr: &str, register: &str) -> bool {
    matches!(
        addr.strip_prefix(register),
        Some(rest) if rest.is_empty() || rest.starts_with(['+', '-'])
    )
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

fn parse_prefixed_reg8_instruction(text: &str) -> Result<Option<Vec<u8>>, Diagnostic> {
    if let Some((dst, src)) = parse_ld_operands(text) {
        if let (Some(dst), Some(src)) = (prefixed_reg8_code(dst), prefixed_reg8_code(src)) {
            return prefixed_reg8_bytes(dst, src, |dst, src| 0x40 + dst * 8 + src).map(Some);
        }
        if let Some(dst) = prefixed_reg8_code(dst) {
            if !is_numeric_literal(src) {
                return Ok(None);
            }
            return prefixed_reg8_unary_bytes(
                dst,
                ld_reg8_imm_opcode(dst.code),
                Some(parse_u8(src)?),
            )
            .map(Some);
        }
    }
    if let Some((inc, register)) = parse_inc_dec_reg8(text) {
        let opcode = if inc { 0x04 } else { 0x05 };
        return Ok(Some(vec![opcode + register * 8]));
    }
    if let Some((inc, operand)) = parse_inc_dec_operand(text) {
        let Some(register) = prefixed_reg8_code(operand.trim()) else {
            return Ok(None);
        };
        let opcode = if inc { 0x04 } else { 0x05 };
        return prefixed_reg8_unary_bytes(register, opcode + register.code * 8, None).map(Some);
    }
    if let Some((op, register)) = parse_accumulator_alu_prefixed_reg8(text) {
        return prefixed_reg8_unary_bytes(
            register,
            accumulator_alu_reg8_opcode(op, register.code),
            None,
        )
        .map(Some);
    }
    Ok(None)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PrefixedReg8 {
    prefix: Option<u8>,
    code: u8,
}

fn prefixed_reg8_code(register: &str) -> Option<PrefixedReg8> {
    match register.trim() {
        "b" => Some(PrefixedReg8 {
            prefix: None,
            code: 0,
        }),
        "c" => Some(PrefixedReg8 {
            prefix: None,
            code: 1,
        }),
        "d" => Some(PrefixedReg8 {
            prefix: None,
            code: 2,
        }),
        "e" => Some(PrefixedReg8 {
            prefix: None,
            code: 3,
        }),
        "h" => Some(PrefixedReg8 {
            prefix: None,
            code: 4,
        }),
        "l" => Some(PrefixedReg8 {
            prefix: None,
            code: 5,
        }),
        "a" => Some(PrefixedReg8 {
            prefix: None,
            code: 7,
        }),
        "ixh" => Some(PrefixedReg8 {
            prefix: Some(0xDD),
            code: 4,
        }),
        "ixl" => Some(PrefixedReg8 {
            prefix: Some(0xDD),
            code: 5,
        }),
        "iyh" => Some(PrefixedReg8 {
            prefix: Some(0xFD),
            code: 4,
        }),
        "iyl" => Some(PrefixedReg8 {
            prefix: Some(0xFD),
            code: 5,
        }),
        _ => None,
    }
}

fn prefixed_reg8_bytes(
    dst: PrefixedReg8,
    src: PrefixedReg8,
    opcode: impl FnOnce(u8, u8) -> u8,
) -> Result<Vec<u8>, Diagnostic> {
    let prefix = merge_reg8_prefixes(dst, src)?;
    let mut bytes = Vec::new();
    if let Some(prefix) = prefix {
        bytes.push(prefix);
    }
    bytes.push(opcode(dst.code, src.code));
    Ok(bytes)
}

fn prefixed_reg8_unary_bytes(
    register: PrefixedReg8,
    opcode: u8,
    immediate: Option<u8>,
) -> Result<Vec<u8>, Diagnostic> {
    let mut bytes = Vec::new();
    if let Some(prefix) = register.prefix {
        bytes.push(prefix);
    }
    bytes.push(opcode);
    if let Some(immediate) = immediate {
        bytes.push(immediate);
    }
    Ok(bytes)
}

fn merge_reg8_prefixes(left: PrefixedReg8, right: PrefixedReg8) -> Result<Option<u8>, Diagnostic> {
    match (left.prefix, right.prefix) {
        (Some(left), Some(right)) if left != right => Err(Diagnostic::new(
            "cannot mix ix and iy 8-bit register aliases in one instruction",
        )),
        (Some(_), None) if matches!(right.code, 4 | 5) => Err(Diagnostic::new(
            "cannot mix ix/iy 8-bit register aliases with h or l",
        )),
        (None, Some(_)) if matches!(left.code, 4 | 5) => Err(Diagnostic::new(
            "cannot mix ix/iy 8-bit register aliases with h or l",
        )),
        (Some(prefix), _) | (_, Some(prefix)) => Ok(Some(prefix)),
        (None, None) => Ok(None),
    }
}

fn parse_accumulator_alu_prefixed_reg8(text: &str) -> Option<(AccumulatorAluOp, PrefixedReg8)> {
    if let Some(src) = text.strip_prefix("add a,") {
        return Some((AccumulatorAluOp::Add, prefixed_reg8_code(src.trim())?));
    }
    if let Some(src) = text.strip_prefix("adc a,") {
        return Some((AccumulatorAluOp::Adc, prefixed_reg8_code(src.trim())?));
    }
    if let Some(src) = text.strip_prefix("sbc a,") {
        return Some((AccumulatorAluOp::Sbc, prefixed_reg8_code(src.trim())?));
    }
    for (prefix, op) in [
        ("sub ", AccumulatorAluOp::Sub),
        ("and ", AccumulatorAluOp::And),
        ("or ", AccumulatorAluOp::Or),
        ("xor ", AccumulatorAluOp::Xor),
        ("cp ", AccumulatorAluOp::Cp),
    ] {
        if let Some(src) = text.strip_prefix(prefix) {
            return Some((op, prefixed_reg8_code(src.trim())?));
        }
    }
    None
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

fn parse_index_instruction(text: &str) -> Result<Option<Vec<u8>>, Diagnostic> {
    if let Some((dst, src)) = parse_ld_operands(text) {
        if let (Some(dst), Some((prefix, offset))) = (reg8_code(dst), parse_index_indirect(src)?) {
            return Ok(Some(vec![prefix, 0x46 + dst * 8, offset]));
        }
        if let (Some((prefix, offset)), Some(src)) = (parse_index_indirect(dst)?, reg8_code(src)) {
            return Ok(Some(vec![prefix, 0x70 + src, offset]));
        }
        if let Some((prefix, offset)) = parse_index_indirect(dst)? {
            if reg8_code(src).is_none() && parse_index_indirect(src)?.is_none() {
                return Ok(Some(vec![prefix, 0x36, offset, parse_u8(src)?]));
            }
        }
    }
    if let Some((inc, operand)) = parse_inc_dec_operand(text) {
        let Some((prefix, offset)) = parse_index_indirect(operand.trim())? else {
            return Ok(None);
        };
        return Ok(Some(vec![prefix, if inc { 0x34 } else { 0x35 }, offset]));
    }
    if let Some((op, prefix, offset)) = parse_index_alu(text)? {
        return Ok(Some(vec![
            prefix,
            accumulator_alu_reg8_opcode(op, 6),
            offset,
        ]));
    }
    Ok(None)
}

fn parse_index_alu(text: &str) -> Result<Option<(AccumulatorAluOp, u8, u8)>, Diagnostic> {
    if let Some(src) = text.strip_prefix("add a,") {
        return parse_index_alu_operand(AccumulatorAluOp::Add, src.trim());
    }
    if let Some(src) = text.strip_prefix("adc a,") {
        return parse_index_alu_operand(AccumulatorAluOp::Adc, src.trim());
    }
    if let Some(src) = text.strip_prefix("sbc a,") {
        return parse_index_alu_operand(AccumulatorAluOp::Sbc, src.trim());
    }
    for (prefix, op) in [
        ("sub ", AccumulatorAluOp::Sub),
        ("and ", AccumulatorAluOp::And),
        ("or ", AccumulatorAluOp::Or),
        ("xor ", AccumulatorAluOp::Xor),
        ("cp ", AccumulatorAluOp::Cp),
    ] {
        if let Some(src) = text.strip_prefix(prefix) {
            return parse_index_alu_operand(op, src.trim());
        }
    }
    Ok(None)
}

fn parse_index_alu_operand(
    op: AccumulatorAluOp,
    operand: &str,
) -> Result<Option<(AccumulatorAluOp, u8, u8)>, Diagnostic> {
    let Some((prefix, offset)) = parse_index_indirect(operand)? else {
        return Ok(None);
    };
    Ok(Some((op, prefix, offset)))
}

fn parse_inc_dec_operand(text: &str) -> Option<(bool, &str)> {
    if let Some(operand) = text.strip_prefix("inc ") {
        return Some((true, operand));
    }
    if let Some(operand) = text.strip_prefix("dec ") {
        return Some((false, operand));
    }
    None
}

fn parse_index_indirect(text: &str) -> Result<Option<(u8, u8)>, Diagnostic> {
    let Some(inner) = parse_wrapped_indirect(text) else {
        return Ok(None);
    };
    if let Some(rest) = inner.strip_prefix("ix") {
        if !is_index_displacement(rest) {
            return Ok(None);
        }
        return parse_index_offset(rest).map(|offset| Some((0xDD, offset)));
    }
    if let Some(rest) = inner.strip_prefix("iy") {
        if !is_index_displacement(rest) {
            return Ok(None);
        }
        return parse_index_offset(rest).map(|offset| Some((0xFD, offset)));
    }
    Ok(None)
}

fn is_index_displacement(text: &str) -> bool {
    let text = text.trim().strip_suffix(')').unwrap_or(text.trim());
    text.is_empty() || text.starts_with(['+', '-'])
}

fn parse_index_offset(text: &str) -> Result<u8, Diagnostic> {
    let text = text.trim();
    let text = text.strip_suffix(')').unwrap_or(text);
    if text.is_empty() {
        return Ok(0);
    }
    let (sign, digits) = text.split_at(1);
    let magnitude = parse_number(digits.trim())?;
    if magnitude > 0x7F {
        return Err(Diagnostic::new(format!(
            "index displacement `{text}` is outside signed 8-bit range"
        )));
    }
    let value = match sign {
        "+" => magnitude as i8,
        "-" => -(magnitude as i8),
        _ => {
            return Err(Diagnostic::new(format!("invalid ix displacement `{text}`")));
        }
    };
    Ok(value as u8)
}

fn parse_index_cb_instruction(text: &str) -> Result<Option<Vec<u8>>, Diagnostic> {
    if let Some((base, operand)) = parse_cb_operation_operand(text) {
        let Some((prefix, offset)) = parse_index_indirect(operand.trim())? else {
            return Ok(None);
        };
        return Ok(Some(vec![prefix, 0xCB, offset, base + 6]));
    }
    let (base, rest) = if let Some(rest) = text.strip_prefix("bit ") {
        (0x40, rest)
    } else if let Some(rest) = text.strip_prefix("res ") {
        (0x80, rest)
    } else if let Some(rest) = text.strip_prefix("set ") {
        (0xC0, rest)
    } else {
        return Ok(None);
    };
    let Some((bit, operand)) = rest.split_once(',') else {
        return Err(Diagnostic::new(format!(
            "invalid bit operation syntax `{text}`"
        )));
    };
    let bit = parse_u8(bit.trim())?;
    if bit > 7 {
        return Err(Diagnostic::new(format!("bit index {bit} is outside 0..7")));
    }
    let Some((prefix, offset)) = parse_index_indirect(operand.trim())? else {
        return Ok(None);
    };
    Ok(Some(vec![prefix, 0xCB, offset, base + bit * 8 + 6]))
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

fn parse_lea_instruction(cpu: AssemblerCpu, text: &str) -> Result<Option<Vec<u8>>, Diagnostic> {
    if !cpu.supports_ez80_syntax() {
        return Ok(None);
    }
    let Some(rest) = text.strip_prefix("lea ") else {
        return Ok(None);
    };
    let Some((dst, src)) = rest.split_once(',') else {
        return Err(Diagnostic::new(format!("invalid lea syntax `{text}`")));
    };
    if dst.trim() != "hl" {
        return Ok(None);
    }
    let Some(rest) = src.trim().strip_prefix("ix") else {
        return Ok(None);
    };
    if !is_index_displacement(rest) {
        return Ok(None);
    }
    Ok(Some(vec![0xED, 0x22, parse_index_offset(rest)?]))
}

fn encode_intel_8080_instruction(
    cpu: AssemblerCpu,
    text: &str,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    if cpu == AssemblerCpu::I8085 {
        match text {
            "rim" => return Ok(Some(vec![0x20])),
            "sim" => return Ok(Some(vec![0x30])),
            _ => {}
        }
    }
    match text {
        "nop" => return Ok(Some(vec![0x00])),
        "hlt" => return Ok(Some(vec![0x76])),
        "ei" => return Ok(Some(vec![0xFB])),
        "di" => return Ok(Some(vec![0xF3])),
        "rlc" => return Ok(Some(vec![0x07])),
        "rrc" => return Ok(Some(vec![0x0F])),
        "ral" => return Ok(Some(vec![0x17])),
        "rar" => return Ok(Some(vec![0x1F])),
        "daa" => return Ok(Some(vec![0x27])),
        "cma" => return Ok(Some(vec![0x2F])),
        "stc" => return Ok(Some(vec![0x37])),
        "cmc" => return Ok(Some(vec![0x3F])),
        "xchg" => return Ok(Some(vec![0xEB])),
        "xthl" => return Ok(Some(vec![0xE3])),
        "sphl" => return Ok(Some(vec![0xF9])),
        "pchl" => return Ok(Some(vec![0xE9])),
        "ret" => return Ok(Some(vec![0xC9])),
        _ => {}
    }
    if let Some((dst, src)) = parse_two_operands(text.strip_prefix("mov ")) {
        let Some(dst) = intel_reg_code(dst) else {
            return Ok(None);
        };
        let Some(src) = intel_reg_code(src) else {
            return Ok(None);
        };
        return Ok(Some(vec![0x40 + dst * 8 + src]));
    }
    if let Some((dst, value)) = parse_two_operands(text.strip_prefix("mvi ")) {
        let Some(dst) = intel_reg_code(dst) else {
            return Ok(None);
        };
        return Ok(Some(vec![0x06 + dst * 8, parse_u8(value)?]));
    }
    if let Some((dst, value)) = parse_two_operands(text.strip_prefix("lxi ")) {
        let Some(dst) = intel_rp_code(dst) else {
            return Ok(None);
        };
        return Ok(Some(word_bytes(0x01 + dst * 0x10, parse_u16(value)?)));
    }
    if let Some(register) = text.strip_prefix("inr ").and_then(intel_reg_code) {
        return Ok(Some(vec![0x04 + register * 8]));
    }
    if let Some(register) = text.strip_prefix("dcr ").and_then(intel_reg_code) {
        return Ok(Some(vec![0x05 + register * 8]));
    }
    if let Some(register) = text.strip_prefix("inx ").and_then(intel_rp_code) {
        return Ok(Some(vec![0x03 + register * 0x10]));
    }
    if let Some(register) = text.strip_prefix("dcx ").and_then(intel_rp_code) {
        return Ok(Some(vec![0x0B + register * 0x10]));
    }
    if let Some(register) = text.strip_prefix("dad ").and_then(intel_rp_code) {
        return Ok(Some(vec![0x09 + register * 0x10]));
    }
    if let Some(register) = text.strip_prefix("push ").and_then(intel_stack_rp_code) {
        return Ok(Some(vec![0xC5 + register * 0x10]));
    }
    if let Some(register) = text.strip_prefix("pop ").and_then(intel_stack_rp_code) {
        return Ok(Some(vec![0xC1 + register * 0x10]));
    }
    if let Some(register) = text.strip_prefix("ldax ").and_then(intel_bd_rp_code) {
        return Ok(Some(vec![0x0A + register * 0x10]));
    }
    if let Some(register) = text.strip_prefix("stax ").and_then(intel_bd_rp_code) {
        return Ok(Some(vec![0x02 + register * 0x10]));
    }
    for (prefix, base) in [
        ("add ", 0x80),
        ("adc ", 0x88),
        ("sub ", 0x90),
        ("sbb ", 0x98),
        ("ana ", 0xA0),
        ("xra ", 0xA8),
        ("ora ", 0xB0),
        ("cmp ", 0xB8),
    ] {
        if let Some(register) = text.strip_prefix(prefix).and_then(intel_reg_code) {
            return Ok(Some(vec![base + register]));
        }
    }
    for (prefix, opcode) in [
        ("adi ", 0xC6),
        ("aci ", 0xCE),
        ("sui ", 0xD6),
        ("sbi ", 0xDE),
        ("ani ", 0xE6),
        ("xri ", 0xEE),
        ("ori ", 0xF6),
        ("cpi ", 0xFE),
        ("in ", 0xDB),
        ("out ", 0xD3),
    ] {
        if let Some(value) = text.strip_prefix(prefix) {
            return Ok(Some(vec![opcode, parse_u8(value)?]));
        }
    }
    for (prefix, opcode) in [
        ("lda ", 0x3A),
        ("sta ", 0x32),
        ("lhld ", 0x2A),
        ("shld ", 0x22),
    ] {
        if let Some(value) = text.strip_prefix(prefix) {
            return Ok(Some(word_bytes(opcode, parse_u16(value)?)));
        }
    }
    if let Some(("rst", value)) = text.split_once(' ') {
        let target = parse_u8(value)?;
        if target > 7 {
            return Err(Diagnostic::new(format!(
                "restart index {target} is outside 0..7"
            )));
        }
        return Ok(Some(vec![0xC7 + target * 8]));
    }
    Ok(None)
}

fn intel_8080_branch_instruction(text: &str) -> Option<BranchInstruction<'_>> {
    for (prefix, opcode) in [
        ("jmp ", 0xC3),
        ("jnz ", 0xC2),
        ("jz ", 0xCA),
        ("jnc ", 0xD2),
        ("jc ", 0xDA),
        ("jpo ", 0xE2),
        ("jpe ", 0xEA),
        ("jp ", 0xF2),
        ("jm ", 0xFA),
        ("call ", 0xCD),
        ("cnz ", 0xC4),
        ("cz ", 0xCC),
        ("cnc ", 0xD4),
        ("cc ", 0xDC),
        ("cpo ", 0xE4),
        ("cpe ", 0xEC),
        ("cp ", 0xF4),
        ("cm ", 0xFC),
    ] {
        if let Some(target) = text.strip_prefix(prefix) {
            return Some(BranchInstruction {
                opcode,
                target: target.trim(),
                width: BranchWidth::Absolute16,
            });
        }
    }
    None
}

fn parse_two_operands(rest: Option<&str>) -> Option<(&str, &str)> {
    let (lhs, rhs) = rest?.split_once(',')?;
    Some((lhs.trim(), rhs.trim()))
}

fn intel_reg_code(register: &str) -> Option<u8> {
    match register.trim() {
        "b" => Some(0),
        "c" => Some(1),
        "d" => Some(2),
        "e" => Some(3),
        "h" => Some(4),
        "l" => Some(5),
        "m" => Some(6),
        "a" => Some(7),
        _ => None,
    }
}

fn intel_rp_code(register: &str) -> Option<u8> {
    match register.trim() {
        "b" => Some(0),
        "d" => Some(1),
        "h" => Some(2),
        "sp" => Some(3),
        _ => None,
    }
}

fn intel_stack_rp_code(register: &str) -> Option<u8> {
    match register.trim() {
        "b" => Some(0),
        "d" => Some(1),
        "h" => Some(2),
        "psw" => Some(3),
        _ => None,
    }
}

fn intel_bd_rp_code(register: &str) -> Option<u8> {
    match register.trim() {
        "b" => Some(0),
        "d" => Some(1),
        _ => None,
    }
}

fn word_bytes(opcode: u8, value: u16) -> Vec<u8> {
    vec![opcode, value as u8, (value >> 8) as u8]
}

fn parse_io_instruction(cpu: AssemblerCpu, text: &str) -> Result<Option<Vec<u8>>, Diagnostic> {
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
        if !matches!(cpu, AssemblerCpu::Z180 | AssemblerCpu::Ez80) {
            return Ok(None);
        }
        let Some((register, port)) = rest.trim().split_once(',') else {
            return Err(Diagnostic::new(format!("invalid in0 syntax `{text}`")));
        };
        let Some(register) = reg8_code(register.trim()) else {
            return Ok(None);
        };
        let port = port
            .trim()
            .strip_prefix('(')
            .and_then(|rest| rest.strip_suffix(')'))
            .ok_or_else(|| Diagnostic::new(format!("invalid in0 syntax `{text}`")))?;
        return Ok(Some(vec![0xED, register * 8, parse_u8(port)?]));
    }
    if let Some(rest) = text.strip_prefix("out0 ") {
        if !matches!(cpu, AssemblerCpu::Z180 | AssemblerCpu::Ez80) {
            return Ok(None);
        }
        let Some((port, register)) = rest.trim().split_once(',') else {
            return Err(Diagnostic::new(format!("invalid out0 syntax `{text}`")));
        };
        let Some(register) = reg8_code(register.trim()) else {
            return Ok(None);
        };
        let port = port
            .trim()
            .strip_prefix('(')
            .and_then(|rest| rest.strip_suffix(')'))
            .ok_or_else(|| Diagnostic::new(format!("invalid out0 syntax `{text}`")))?;
        return Ok(Some(vec![0xED, 0x01 + register * 8, parse_u8(port)?]));
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

fn parse_u16(text: &str) -> Result<u16, Diagnostic> {
    let value = parse_number(text)?;
    if value > 0xFFFF {
        return Err(Diagnostic::new(format!(
            "value {text} is outside u16 range"
        )));
    }
    Ok(value as u16)
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
    use ez80::Machine;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn assert_ez80_emulator_decodes(syntax: &str, bytes: &[u8]) {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut machine = ez80::PlainMachine::new();
            let mut cpu = ez80::Cpu::new_ez80();
            for (offset, byte) in bytes.iter().copied().enumerate() {
                machine.poke(offset as u32, byte);
            }
            cpu.disasm_instruction(&mut machine)
        }));
        let disasm = result
            .unwrap_or_else(|_| panic!("emulator failed to decode `{syntax}` bytes {bytes:02X?}"));
        assert_ne!(
            disasm, "ILLEGAL",
            "emulator decoded `{syntax}` bytes {bytes:02X?} as illegal"
        );
    }

    #[test]
    fn exact_instruction_metadata_encodes_common_ops() {
        assert_eq!(
            exact_instruction(AssemblerCpu::Ez80, "nop").unwrap().bytes,
            &[0x00]
        );
        assert_eq!(
            exact_instruction(AssemblerCpu::Ez80, "reti").unwrap().bytes,
            &[0xED, 0x4D]
        );
    }

    #[test]
    fn metadata_can_generate_z80_subset() {
        let z80 = instruction_set(AssemblerCpu::Z80).collect::<Vec<_>>();
        assert!(z80.iter().any(|instruction| instruction.syntax == "ret"));
        assert!(z80.iter().any(|instruction| instruction.syntax == "im 2"));
    }

    #[test]
    fn ez80_emulator_decodes_all_exact_instruction_metadata() {
        for instruction in instruction_set(AssemblerCpu::Ez80) {
            assert_ez80_emulator_decodes(instruction.syntax, instruction.bytes);
        }
    }

    #[test]
    fn ez80_emulator_decodes_representative_generated_instruction_metadata() {
        let cases = [
            "ld b, a",
            "ld a, 7Fh",
            "inc c",
            "add a, c",
            "inc hl",
            "add hl, de",
            "srl a",
            "bit 3, (hl)",
            "in a, (34h)",
            "out0 (0Ch), a",
            "rst.lis 10h",
            "xor 55h",
            "ld d, (hl)",
            "ld (hl), 43h",
            "ld c, (ix+2)",
            "ld (iy-1), e",
            "ld (ix+4), 99h",
            "xor (iy+0)",
            "rlc (ix+1)",
            "rr (iy-2)",
            "bit 3, (ix+4)",
            "res 2, (iy+5)",
            "set 7, (ix-6)",
        ];

        for syntax in cases {
            let bytes = encode_generated_instruction(AssemblerCpu::Ez80, syntax)
                .unwrap()
                .unwrap_or_else(|| panic!("missing generated encoding for `{syntax}`"));
            assert_ez80_emulator_decodes(syntax, &bytes);
        }
    }

    #[test]
    fn generated_instruction_metadata_encodes_operand_families() {
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "ld b, a").unwrap(),
            Some(vec![0x47])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "ld a, 7Fh").unwrap(),
            Some(vec![0x3E, 0x7F])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "inc c").unwrap(),
            Some(vec![0x0C])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "add a, c").unwrap(),
            Some(vec![0x81])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "inc hl").unwrap(),
            Some(vec![0x23])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "add hl, de").unwrap(),
            Some(vec![0x19])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "srl a").unwrap(),
            Some(vec![0xCB, 0x3F])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "bit 3, (hl)").unwrap(),
            Some(vec![0xCB, 0x5E])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "in a, (34h)").unwrap(),
            Some(vec![0xDB, 0x34])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "out0 (0Ch), a").unwrap(),
            Some(vec![0xED, 0x39, 0x0C])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "rst.lis 10h").unwrap(),
            Some(vec![0x49, 0xD7])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "xor 55h").unwrap(),
            Some(vec![0xEE, 0x55])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "ld d, (hl)").unwrap(),
            Some(vec![0x56])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "ld (hl), 43h").unwrap(),
            Some(vec![0x36, 0x43])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "ld c, (ix+2)").unwrap(),
            Some(vec![0xDD, 0x4E, 0x02])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "ld (iy-1), e").unwrap(),
            Some(vec![0xFD, 0x73, 0xFF])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "ld (ix+4), 99h").unwrap(),
            Some(vec![0xDD, 0x36, 0x04, 0x99])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "xor (iy+0)").unwrap(),
            Some(vec![0xFD, 0xAE, 0x00])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "rlc (ix+1)").unwrap(),
            Some(vec![0xDD, 0xCB, 0x01, 0x06])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "rr (iy-2)").unwrap(),
            Some(vec![0xFD, 0xCB, 0xFE, 0x1E])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "bit 3, (ix+4)").unwrap(),
            Some(vec![0xDD, 0xCB, 0x04, 0x5E])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "res 2, (iy+5)").unwrap(),
            Some(vec![0xFD, 0xCB, 0x05, 0x96])
        );
        assert_eq!(
            encode_generated_instruction(AssemblerCpu::Ez80, "set 7, (ix-6)").unwrap(),
            Some(vec![0xDD, 0xCB, 0xFA, 0xFE])
        );
    }

    #[test]
    fn generated_instruction_metadata_encodes_ix_iy_byte_aliases() {
        let cases = [
            ("ld ixh, 12h", vec![0xDD, 0x26, 0x12]),
            ("ld ixl, a", vec![0xDD, 0x6F]),
            ("ld b, ixh", vec![0xDD, 0x44]),
            ("ld ixh, ixl", vec![0xDD, 0x65]),
            ("inc ixh", vec![0xDD, 0x24]),
            ("dec ixl", vec![0xDD, 0x2D]),
            ("add a, ixh", vec![0xDD, 0x84]),
            ("xor ixl", vec![0xDD, 0xAD]),
            ("ld iyh, 34h", vec![0xFD, 0x26, 0x34]),
            ("ld iyl, a", vec![0xFD, 0x6F]),
            ("ld c, iyh", vec![0xFD, 0x4C]),
            ("ld iyh, iyl", vec![0xFD, 0x65]),
            ("inc iyh", vec![0xFD, 0x24]),
            ("dec iyl", vec![0xFD, 0x2D]),
            ("adc a, iyh", vec![0xFD, 0x8C]),
            ("cp iyl", vec![0xFD, 0xBD]),
        ];

        for (syntax, bytes) in cases {
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
                Some(bytes.clone()),
                "{syntax}"
            );
            assert_ez80_emulator_decodes(syntax, &bytes);
        }
    }

    #[test]
    fn rejects_misleading_ix_iy_byte_alias_mixes() {
        let cases = ["ld ixh, iyh", "ld h, ixh", "ld iyl, l"];

        for syntax in cases {
            let error = encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap_err();
            assert!(
                error.message.contains("cannot mix"),
                "{syntax}: {}",
                error.message
            );
        }
    }

    #[test]
    fn generated_instruction_metadata_encodes_all_in0_out0_register_forms() {
        let cases = [
            ("in0 b, (12h)", vec![0xED, 0x00, 0x12]),
            ("in0 c, (12h)", vec![0xED, 0x08, 0x12]),
            ("in0 d, (12h)", vec![0xED, 0x10, 0x12]),
            ("in0 e, (12h)", vec![0xED, 0x18, 0x12]),
            ("in0 h, (12h)", vec![0xED, 0x20, 0x12]),
            ("in0 l, (12h)", vec![0xED, 0x28, 0x12]),
            ("in0 a, (12h)", vec![0xED, 0x38, 0x12]),
            ("out0 (34h), b", vec![0xED, 0x01, 0x34]),
            ("out0 (34h), c", vec![0xED, 0x09, 0x34]),
            ("out0 (34h), d", vec![0xED, 0x11, 0x34]),
            ("out0 (34h), e", vec![0xED, 0x19, 0x34]),
            ("out0 (34h), h", vec![0xED, 0x21, 0x34]),
            ("out0 (34h), l", vec![0xED, 0x29, 0x34]),
            ("out0 (34h), a", vec![0xED, 0x39, 0x34]),
        ];

        for (syntax, bytes) in cases {
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
                Some(bytes.clone()),
                "{syntax}"
            );
            assert_ez80_emulator_decodes(syntax, &bytes);
        }
    }

    #[test]
    fn generated_instruction_metadata_encodes_z80n_extensions() {
        let cases = [
            ("swapnib", vec![0xED, 0x23]),
            ("mirror a", vec![0xED, 0x24]),
            ("test 7Fh", vec![0xED, 0x27, 0x7F]),
            ("bsla de,b", vec![0xED, 0x28]),
            ("bsra de,b", vec![0xED, 0x29]),
            ("bsrl de,b", vec![0xED, 0x2A]),
            ("bsrf de,b", vec![0xED, 0x2B]),
            ("brlc de,b", vec![0xED, 0x2C]),
            ("mul d,e", vec![0xED, 0x30]),
            ("add hl,a", vec![0xED, 0x31]),
            ("add de,a", vec![0xED, 0x32]),
            ("add bc,a", vec![0xED, 0x33]),
            ("add hl,1234h", vec![0xED, 0x34, 0x34, 0x12]),
            ("add de,2345h", vec![0xED, 0x35, 0x45, 0x23]),
            ("add bc,3456h", vec![0xED, 0x36, 0x56, 0x34]),
            ("push 1234h", vec![0xED, 0x8A, 0x12, 0x34]),
            ("outinb", vec![0xED, 0x90]),
            ("nextreg 12h,34h", vec![0xED, 0x91, 0x12, 0x34]),
            ("nextreg 12h,a", vec![0xED, 0x92, 0x12]),
            ("pixeldn", vec![0xED, 0x93]),
            ("pixelad", vec![0xED, 0x94]),
            ("setae", vec![0xED, 0x95]),
            ("jp (c)", vec![0xED, 0x98]),
            ("ldix", vec![0xED, 0xA4]),
            ("ldws", vec![0xED, 0xA5]),
            ("lddx", vec![0xED, 0xAC]),
            ("ldirx", vec![0xED, 0xB4]),
            ("ldpirx", vec![0xED, 0xB7]),
            ("lddrx", vec![0xED, 0xBC]),
        ];

        for (syntax, bytes) in cases {
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Z80N, syntax).unwrap(),
                Some(bytes),
                "{syntax}"
            );
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Z80, syntax).unwrap(),
                None,
                "{syntax}"
            );
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Z180, syntax).unwrap(),
                None,
                "{syntax}"
            );
        }
    }

    #[test]
    fn generated_instruction_metadata_encodes_z180_extensions() {
        let cases = [
            ("slp", vec![0xED, 0x76], true),
            ("mlt bc", vec![0xED, 0x4C], true),
            ("otim", vec![0xED, 0x83], false),
            ("otimr", vec![0xED, 0x93], false),
            ("otdm", vec![0xED, 0x8B], false),
            ("otdmr", vec![0xED, 0x9B], false),
            ("tst b", vec![0xED, 0x04], false),
            ("tst c", vec![0xED, 0x0C], false),
            ("tst (hl)", vec![0xED, 0x34], false),
            ("tst a", vec![0xED, 0x3C], false),
            ("tst 5Ah", vec![0xED, 0x64, 0x5A], false),
            ("tstio 80h", vec![0xED, 0x74, 0x80], false),
        ];

        for (syntax, bytes, also_ez80) in cases {
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Z180, syntax).unwrap(),
                Some(bytes.clone()),
                "{syntax}"
            );
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Z80, syntax).unwrap(),
                None,
                "{syntax}"
            );
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Z80N, syntax).unwrap(),
                None,
                "{syntax}"
            );
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
                also_ez80.then_some(bytes),
                "{syntax}"
            );
        }
    }

    #[test]
    fn direct24_metadata_encodes_sp_loads_and_stores() {
        assert_eq!(
            direct24_instruction(AssemblerCpu::Ez80, "ld sp, (040000h)").unwrap(),
            Direct24Instruction {
                prefix: &[0xED, 0x7B],
                addr: "040000h",
            }
        );
        assert_eq!(
            direct24_instruction(AssemblerCpu::Ez80, "ld (040003h), sp").unwrap(),
            Direct24Instruction {
                prefix: &[0xED, 0x73],
                addr: "040003h",
            }
        );
        assert_ez80_emulator_decodes("ld sp, (040000h)", &[0xED, 0x7B, 0x00, 0x00, 0x04]);
        assert_ez80_emulator_decodes("ld (040003h), sp", &[0xED, 0x73, 0x03, 0x00, 0x04]);
    }

    #[test]
    fn generated_instruction_metadata_encodes_ez80_mode_suffixes() {
        let cases = [
            ("nop.sis", vec![0x40, 0x00]),
            ("ld.lis b, a", vec![0x49, 0x47]),
            ("xor.sil 55h", vec![0x52, 0xEE, 0x55]),
            ("out0.lil (0Ch), a", vec![0x5B, 0xED, 0x39, 0x0C]),
        ];

        for (syntax, bytes) in cases {
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Ez80, syntax).unwrap(),
                Some(bytes.clone()),
                "{syntax}"
            );
            assert_ez80_emulator_decodes(syntax, &bytes);
            assert_eq!(
                generated_instruction_len(AssemblerCpu::Ez80, syntax).unwrap(),
                Some(bytes.len()),
                "{syntax}"
            );
            assert_eq!(
                encode_generated_instruction(AssemblerCpu::Z80, syntax).unwrap(),
                None,
                "{syntax}"
            );
        }
    }

    #[test]
    fn branch_metadata_describes_control_flow_widths() {
        let call = branch_instruction(AssemblerCpu::Ez80, "call nz, _main").unwrap();
        assert_eq!(call.opcode, 0xC4);
        assert_eq!(call.target, "_main");
        assert_eq!(call.len(), 4);

        let jr = branch_instruction(AssemblerCpu::Ez80, "jr z, .done").unwrap();
        assert_eq!(jr.opcode, 0x28);
        assert_eq!(jr.target, ".done");
        assert_eq!(jr.len(), 2);
    }
}
