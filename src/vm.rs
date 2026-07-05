use std::{
    cell::Cell,
    collections::{BTreeMap, HashMap},
    panic::{AssertUnwindSafe, catch_unwind},
};

use ez80::{Cpu, Machine, Reg16};

use crate::diagnostic::Diagnostic;
use crate::target::{Address24, EZRA_LOAD_ADDR, EZRA_STACK_TOP};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestRun {
    pub halted: bool,
    pub result_code: u8,
    pub instructions: u64,
    pub debug_output: Vec<u8>,
    pub ports: [u8; 256],
    pub failure: Option<TestRunFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TestRunFailure {
    Timeout,
    ExecutionOutsideMappedMemory { pc: u32 },
    IllegalInstruction { pc: u32 },
    StackOverflow { sp: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssembledProgram {
    pub bytes: Vec<u8>,
    pub symbols: Vec<AssemblySymbol>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssemblySymbol {
    pub name: String,
    pub addr: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestRunOptions {
    pub instruction_budget: u64,
    pub initial_ports: Vec<(u8, u8)>,
    pub initial_memory: Vec<(u32, u8)>,
    pub stack_top: u32,
}

const TEST_STACK_BYTES: u32 = 0x010000;

pub fn run_assembly_test(assembly: &str, instruction_budget: u64) -> Result<TestRun, Diagnostic> {
    run_assembly_test_with_options(
        assembly,
        &TestRunOptions {
            instruction_budget,
            initial_ports: Vec::new(),
            initial_memory: Vec::new(),
            stack_top: EZRA_STACK_TOP.get(),
        },
    )
}

pub fn run_assembly_test_with_options(
    assembly: &str,
    options: &TestRunOptions,
) -> Result<TestRun, Diagnostic> {
    run_assembly_test_with_options_at(assembly, options, EZRA_LOAD_ADDR.get())
}

pub fn run_assembly_test_with_options_at(
    assembly: &str,
    options: &TestRunOptions,
    base_addr: u32,
) -> Result<TestRun, Diagnostic> {
    if options.stack_top > Address24::MAX {
        return Err(Diagnostic::new(format!(
            "test stack top 0x{:X} is outside the 24-bit address space",
            options.stack_top
        )));
    }
    for (address, _) in &options.initial_memory {
        if *address > Address24::MAX {
            return Err(Diagnostic::new(format!(
                "test memory address 0x{address:X} is outside the 24-bit address space"
            )));
        }
    }

    let code = assemble_ez80_subset_at(assembly, base_addr)?;
    let code_start = base_addr;
    let code_end = checked_code_end(code_start, code.len())?;
    let mut machine = TestMachine::new();
    for (port, value) in &options.initial_ports {
        machine.ports[*port as usize] = *value;
    }
    for (address, value) in &options.initial_memory {
        machine.poke(*address, *value);
    }
    for (address, byte) in code.into_iter().enumerate() {
        machine.poke(base_addr + address as u32, byte);
    }

    let mut cpu = Cpu::new_ez80();
    cpu.state.reg.adl = true;
    cpu.state.set_pc(base_addr);
    cpu.state.reg.set24(Reg16::SP, options.stack_top);
    if std::env::var_os("EZRA_TRACE_VM").is_some() {
        cpu.set_trace(true);
    }

    for instruction in 0..options.instruction_budget {
        let pc = cpu.state.pc();
        if pc < code_start || pc >= code_end {
            return Ok(TestRun {
                halted: false,
                result_code: machine.result_code,
                instructions: instruction,
                debug_output: machine.debug_output,
                ports: machine.ports,
                failure: Some(TestRunFailure::ExecutionOutsideMappedMemory { pc }),
            });
        }
        if catch_unwind(AssertUnwindSafe(|| cpu.execute_instruction(&mut machine))).is_err() {
            return Ok(TestRun {
                halted: false,
                result_code: machine.result_code,
                instructions: instruction,
                debug_output: machine.debug_output,
                ports: machine.ports,
                failure: Some(TestRunFailure::IllegalInstruction { pc }),
            });
        }
        let sp = cpu.state.reg.get24(Reg16::SP);
        if !stack_pointer_in_bounds(sp, options.stack_top) {
            return Ok(TestRun {
                halted: false,
                result_code: machine.result_code,
                instructions: instruction + 1,
                debug_output: machine.debug_output,
                ports: machine.ports,
                failure: Some(TestRunFailure::StackOverflow { sp }),
            });
        }
        if machine.halted {
            return Ok(TestRun {
                halted: true,
                result_code: machine.result_code,
                instructions: instruction + 1,
                debug_output: machine.debug_output,
                ports: machine.ports,
                failure: None,
            });
        }
    }

    Ok(TestRun {
        halted: false,
        result_code: machine.result_code,
        instructions: options.instruction_budget,
        debug_output: machine.debug_output,
        ports: machine.ports,
        failure: Some(TestRunFailure::Timeout),
    })
}

fn stack_pointer_in_bounds(sp: u32, stack_top: u32) -> bool {
    let floor = stack_top.saturating_sub(TEST_STACK_BYTES);
    (floor..=stack_top).contains(&sp)
}

pub fn assemble_ez80_subset_at(assembly: &str, base_addr: u32) -> Result<Vec<u8>, Diagnostic> {
    Ok(assemble_ez80_subset_with_symbols_at(assembly, base_addr)?.bytes)
}

pub fn assemble_ez80_subset_with_symbols_at(
    assembly: &str,
    base_addr: u32,
) -> Result<AssembledProgram, Diagnostic> {
    if base_addr > Address24::MAX {
        return Err(Diagnostic::new(format!(
            "assembly base address 0x{base_addr:X} is outside the 24-bit address space"
        )));
    }
    let instructions = assembly.lines().filter_map(parse_line).collect::<Vec<_>>();
    let mut labels = BTreeMap::new();
    let mut pc = base_addr & 0xFF_FFFF;

    for instruction in &instructions {
        match instruction {
            AsmLine::Label(name) => {
                if labels.insert(name.clone(), pc).is_some() {
                    return Err(Diagnostic::new(format!(
                        "duplicate assembly label `{name}`"
                    )));
                }
            }
            AsmLine::Instruction(text) => pc += instruction_len(text)? as u32,
        }
    }

    let symbols = labels
        .iter()
        .map(|(name, addr)| AssemblySymbol {
            name: name.clone(),
            addr: *addr,
        })
        .collect();
    let labels = labels.into_iter().collect::<HashMap<_, _>>();
    let mut bytes = Vec::new();
    let mut pc = base_addr & 0xFF_FFFF;
    for instruction in instructions {
        if let AsmLine::Instruction(text) = instruction {
            emit_instruction(&text, &labels, pc, &mut bytes)?;
            pc = (pc + instruction_len(&text)? as u32) & 0xFF_FFFF;
        }
    }
    Ok(AssembledProgram { bytes, symbols })
}

fn checked_code_end(base_addr: u32, len: usize) -> Result<u32, Diagnostic> {
    let len = u32::try_from(len)
        .map_err(|_| Diagnostic::new("test program exceeds the 24-bit address space"))?;
    let end = base_addr
        .checked_add(len)
        .ok_or_else(|| Diagnostic::new("test program exceeds the 24-bit address space"))?;
    if end > Address24::MAX + 1 {
        return Err(Diagnostic::new(format!(
            "test program at 0x{base_addr:06X} with length 0x{len:X} exceeds the 24-bit address space"
        )));
    }
    Ok(end)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AsmLine {
    Label(String),
    Instruction(String),
}

fn parse_line(line: &str) -> Option<AsmLine> {
    let line = line.split(';').next().unwrap_or("").trim();
    if line.is_empty() || line.starts_with("section ") {
        return None;
    }
    if let Some(label) = line.strip_suffix(':') {
        return Some(AsmLine::Label(label.to_owned()));
    }
    Some(AsmLine::Instruction(line.to_owned()))
}

fn instruction_len(text: &str) -> Result<usize, Diagnostic> {
    if text.starts_with("ld sp,")
        || text.starts_with("call ")
        || text.starts_with("jp z,")
        || text.starts_with("jp nz,")
        || text.starts_with("jp c,")
        || text.starts_with("jp nc,")
        || text.starts_with("jp ")
    {
        Ok(4)
    } else if text.starts_with("jr z,")
        || text.starts_with("jr nz,")
        || text.starts_with("jr c,")
        || text.starts_with("jr nc,")
        || text.starts_with("jr ")
        || text.starts_with("djnz ")
    {
        Ok(2)
    } else if matches!(
        text,
        "ret"
            | "ret nz"
            | "ret z"
            | "ret nc"
            | "ret c"
            | "nop"
            | "di"
            | "ei"
            | "or a"
            | "ex de, hl"
            | "push af"
            | "push bc"
            | "push de"
            | "push hl"
            | "pop af"
            | "pop bc"
            | "pop de"
            | "pop hl"
            | "dec sp"
            | "inc sp"
            | "inc b"
            | "dec b"
            | "dec c"
            | "ld b, a"
            | "ld c, a"
            | "ld d, a"
            | "ld a, b"
            | "ld a, c"
            | "ld a, d"
            | "ld a, h"
            | "ld a, l"
            | "ld a, (bc)"
            | "ld a, (hl)"
            | "ld a, (de)"
            | "ld (bc), a"
            | "ld (de), a"
            | "ld h, b"
            | "ld h, a"
            | "ld l, c"
            | "ld l, a"
            | "ld (hl), a"
            | "inc hl"
            | "inc de"
            | "dec bc"
            | "add hl, hl"
            | "add hl, bc"
            | "add hl, de"
            | "add hl, sp"
            | "add a, a"
            | "add a, b"
            | "add a, c"
            | "sub b"
            | "sub c"
            | "and b"
            | "and c"
            | "or b"
            | "or c"
            | "xor b"
            | "xor c"
            | "xor a"
            | "cp b"
            | "cp c"
    ) {
        Ok(1)
    } else if matches!(
        text,
        "reti" | "sra a" | "srl a" | "rl a" | "rr a" | "push ix" | "pop ix" | "push iy" | "pop iy"
    ) {
        Ok(2)
    } else if parse_block_operation(text).is_some() {
        Ok(2)
    } else if parse_mlt_reg16(text).is_some() {
        Ok(2)
    } else if text == "sbc hl, bc"
        || text == "sbc hl, de"
        || text == "add ix, sp"
        || text == "add iy, sp"
    {
        Ok(2)
    } else if is_index_byte_load_or_store(text) {
        Ok(3)
    } else if parse_ld_reg8_from_hl(text).is_some() || parse_ld_hl_from_reg8(text).is_some() {
        Ok(1)
    } else if parse_ld_hl_imm(text)?.is_some() {
        Ok(2)
    } else if parse_ld_reg8_reg8(text).is_some() {
        Ok(1)
    } else if parse_ld_reg8_imm(text)?.is_some() {
        Ok(2)
    } else if parse_inc_dec_reg8(text).is_some() {
        Ok(1)
    } else if parse_accumulator_alu_reg8(text).is_some() {
        Ok(1)
    } else if parse_accumulator_alu_imm(text)?.is_some() {
        Ok(2)
    } else if parse_ld_reg16_direct_load(text).is_some()
        || parse_ld_direct_reg16_store(text).is_some()
    {
        Ok(5)
    } else if text.starts_with("ld hl, (")
        || text.starts_with("ld a, (")
        || text.starts_with("ld (")
    {
        Ok(4)
    } else if text.starts_with("ld ix,") || text.starts_with("ld iy,") {
        Ok(5)
    } else if text.starts_with("ld hl,") || text.starts_with("ld de,") || text.starts_with("ld bc,")
    {
        Ok(4)
    } else if text.starts_with("ld h,") || text.starts_with("ld a,") {
        Ok(2)
    } else if text.starts_with("xor ") {
        Ok(2)
    } else if text.starts_with("in0 ") || text.starts_with("out0 ") {
        Ok(3)
    } else {
        Err(Diagnostic::new(format!(
            "test assembler does not support instruction `{text}`"
        )))
    }
}

fn emit_instruction(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    bytes: &mut Vec<u8>,
) -> Result<(), Diagnostic> {
    if let Some(value) = text.strip_prefix("ld sp,") {
        bytes.push(0x31);
        push24(bytes, parse_addr(value.trim(), labels, pc)?);
    } else if let Some(target) = text.strip_prefix("call ") {
        bytes.push(0xCD);
        push24(bytes, parse_addr(target.trim(), labels, pc)?);
    } else if let Some(target) = text.strip_prefix("jp z,") {
        bytes.push(0xCA);
        push24(bytes, parse_addr(target.trim(), labels, pc)?);
    } else if let Some(target) = text.strip_prefix("jp nz,") {
        bytes.push(0xC2);
        push24(bytes, parse_addr(target.trim(), labels, pc)?);
    } else if let Some(target) = text.strip_prefix("jp c,") {
        bytes.push(0xDA);
        push24(bytes, parse_addr(target.trim(), labels, pc)?);
    } else if let Some(target) = text.strip_prefix("jp nc,") {
        bytes.push(0xD2);
        push24(bytes, parse_addr(target.trim(), labels, pc)?);
    } else if let Some(target) = text.strip_prefix("jp ") {
        bytes.push(0xC3);
        push24(bytes, parse_addr(target.trim(), labels, pc)?);
    } else if let Some(target) = text.strip_prefix("jr z,") {
        bytes.push(0x28);
        bytes.push(relative_offset(pc, parse_addr(target.trim(), labels, pc)?)?);
    } else if let Some(target) = text.strip_prefix("jr nz,") {
        bytes.push(0x20);
        bytes.push(relative_offset(pc, parse_addr(target.trim(), labels, pc)?)?);
    } else if let Some(target) = text.strip_prefix("jr c,") {
        bytes.push(0x38);
        bytes.push(relative_offset(pc, parse_addr(target.trim(), labels, pc)?)?);
    } else if let Some(target) = text.strip_prefix("jr nc,") {
        bytes.push(0x30);
        bytes.push(relative_offset(pc, parse_addr(target.trim(), labels, pc)?)?);
    } else if let Some(target) = text.strip_prefix("jr ") {
        bytes.push(0x18);
        bytes.push(relative_offset(pc, parse_addr(target.trim(), labels, pc)?)?);
    } else if let Some(target) = text.strip_prefix("djnz ") {
        bytes.push(0x10);
        bytes.push(relative_offset(pc, parse_addr(target.trim(), labels, pc)?)?);
    } else if let Some((index, offset)) = parse_index_byte_load(text)? {
        bytes.extend([index.prefix(), 0x7E, offset]);
    } else if let Some((index, offset)) = parse_index_byte_store(text)? {
        bytes.extend([index.prefix(), 0x77, offset]);
    } else if let Some(register) = parse_ld_reg8_from_hl(text) {
        bytes.push(0x46 + register * 8);
    } else if let Some(register) = parse_ld_hl_from_reg8(text) {
        bytes.push(0x70 + register);
    } else if let Some(value) = parse_ld_hl_imm(text)? {
        bytes.push(0x36);
        bytes.push(value);
    } else if text == "ld a, (de)" {
        bytes.push(0x1A);
    } else if text == "ld a, (bc)" {
        bytes.push(0x0A);
    } else if text == "ld (de), a" {
        bytes.push(0x12);
    } else if text == "ld (bc), a" {
        bytes.push(0x02);
    } else if let Some((register, addr)) = parse_ld_reg16_direct_load(text) {
        bytes.extend([0xED, ld_reg16_direct_load_opcode(register)]);
        push24(bytes, parse_addr(addr, labels, pc)?);
    } else if let Some((addr, register)) = parse_ld_direct_reg16_store(text) {
        bytes.extend([0xED, ld_direct_reg16_store_opcode(register)]);
        push24(bytes, parse_addr(addr, labels, pc)?);
    } else if let Some(rest) = text.strip_prefix("ld hl, (") {
        let addr = rest
            .strip_suffix(')')
            .ok_or_else(|| Diagnostic::new(format!("invalid load syntax `{text}`")))?;
        bytes.push(0x2A);
        push24(bytes, parse_addr(addr, labels, pc)?);
    } else if let Some(rest) = text.strip_prefix("ld a, (") {
        let addr = rest
            .strip_suffix(')')
            .ok_or_else(|| Diagnostic::new(format!("invalid load syntax `{text}`")))?;
        bytes.push(0x3A);
        push24(bytes, parse_addr(addr, labels, pc)?);
    } else if let Some(rest) = text.strip_prefix("ld (") {
        if let Some(addr) = rest.strip_suffix("), a") {
            bytes.push(0x32);
            push24(bytes, parse_addr(addr, labels, pc)?);
        } else if let Some(addr) = rest.strip_suffix("), hl") {
            bytes.push(0x22);
            push24(bytes, parse_addr(addr, labels, pc)?);
        } else {
            return Err(Diagnostic::new(format!("invalid store syntax `{text}`")));
        }
    } else if let Some(value) = text.strip_prefix("ld hl,") {
        bytes.push(0x21);
        push24(bytes, parse_addr(value.trim(), labels, pc)?);
    } else if let Some(value) = text.strip_prefix("ld de,") {
        bytes.push(0x11);
        push24(bytes, parse_addr(value.trim(), labels, pc)?);
    } else if let Some(value) = text.strip_prefix("ld bc,") {
        bytes.push(0x01);
        push24(bytes, parse_addr(value.trim(), labels, pc)?);
    } else if let Some((dst, src)) = parse_ld_reg8_reg8(text) {
        bytes.push(0x40 + dst * 8 + src);
    } else if let Some((dst, value)) = parse_ld_reg8_imm(text)? {
        bytes.push(ld_reg8_imm_opcode(dst));
        bytes.push(value);
    } else if let Some((inc, register)) = parse_inc_dec_reg8(text) {
        bytes.push(inc_dec_reg8_opcode(inc, register));
    } else if let Some((op, register)) = parse_accumulator_alu_reg8(text) {
        bytes.push(accumulator_alu_reg8_opcode(op, register));
    } else if let Some((op, value)) = parse_accumulator_alu_imm(text)? {
        bytes.push(accumulator_alu_imm_opcode(op));
        bytes.push(value);
    } else if let Some(rest) = text.strip_prefix("in0 ") {
        let port = rest
            .trim()
            .strip_prefix("a, (")
            .and_then(|rest| rest.strip_suffix(')'))
            .ok_or_else(|| Diagnostic::new(format!("invalid in0 syntax `{text}`")))?;
        bytes.extend([0xED, 0x38, parse_u8(port)?]);
    } else if let Some(rest) = text.strip_prefix("out0 ") {
        let port = rest
            .trim()
            .strip_prefix('(')
            .and_then(|rest| rest.split_once(')'))
            .ok_or_else(|| Diagnostic::new(format!("invalid out0 syntax `{text}`")))?
            .0;
        bytes.extend([0xED, 0x39, parse_u8(port)?]);
    } else if text == "ret" {
        bytes.push(0xC9);
    } else if text == "ret nz" {
        bytes.push(0xC0);
    } else if text == "ret z" {
        bytes.push(0xC8);
    } else if text == "ret nc" {
        bytes.push(0xD0);
    } else if text == "ret c" {
        bytes.push(0xD8);
    } else if text == "nop" {
        bytes.push(0x00);
    } else if text == "di" {
        bytes.push(0xF3);
    } else if text == "ei" {
        bytes.push(0xFB);
    } else if text == "or a" {
        bytes.push(0xB7);
    } else if text == "ex de, hl" {
        bytes.push(0xEB);
    } else if text == "push af" {
        bytes.push(0xF5);
    } else if text == "push bc" {
        bytes.push(0xC5);
    } else if text == "push de" {
        bytes.push(0xD5);
    } else if text == "push hl" {
        bytes.push(0xE5);
    } else if text == "push ix" {
        bytes.extend([0xDD, 0xE5]);
    } else if text == "push iy" {
        bytes.extend([0xFD, 0xE5]);
    } else if text == "pop af" {
        bytes.push(0xF1);
    } else if text == "pop bc" {
        bytes.push(0xC1);
    } else if text == "pop de" {
        bytes.push(0xD1);
    } else if text == "pop hl" {
        bytes.push(0xE1);
    } else if text == "dec sp" {
        bytes.push(0x3B);
    } else if text == "inc sp" {
        bytes.push(0x33);
    } else if text == "pop ix" {
        bytes.extend([0xDD, 0xE1]);
    } else if text == "pop iy" {
        bytes.extend([0xFD, 0xE1]);
    } else if text == "reti" {
        bytes.extend([0xED, 0x4D]);
    } else if let Some(opcode) = parse_block_operation(text) {
        bytes.extend([0xED, opcode]);
    } else if let Some(opcode) = parse_mlt_reg16(text) {
        bytes.extend([0xED, opcode]);
    } else if text == "add ix, sp" {
        bytes.extend([0xDD, 0x39]);
    } else if text == "add iy, sp" {
        bytes.extend([0xFD, 0x39]);
    } else if text == "add hl, sp" {
        bytes.push(0x39);
    } else if text == "inc b" {
        bytes.push(0x04);
    } else if text == "dec b" {
        bytes.push(0x05);
    } else if text == "dec c" {
        bytes.push(0x0D);
    } else if let Some(value) = text.strip_prefix("ld ix,") {
        bytes.extend([0xDD, 0x21]);
        push24(bytes, parse_addr(value.trim(), labels, pc)?);
    } else if let Some(value) = text.strip_prefix("ld iy,") {
        bytes.extend([0xFD, 0x21]);
        push24(bytes, parse_addr(value.trim(), labels, pc)?);
    } else if text == "ld b, a" {
        bytes.push(0x47);
    } else if text == "ld c, a" {
        bytes.push(0x4F);
    } else if text == "ld d, a" {
        bytes.push(0x57);
    } else if text == "ld a, b" {
        bytes.push(0x78);
    } else if text == "ld a, c" {
        bytes.push(0x79);
    } else if text == "ld a, d" {
        bytes.push(0x7A);
    } else if text == "ld a, h" {
        bytes.push(0x7C);
    } else if text == "ld a, l" {
        bytes.push(0x7D);
    } else if text == "ld h, b" {
        bytes.push(0x60);
    } else if text == "ld h, a" {
        bytes.push(0x67);
    } else if let Some(value) = text.strip_prefix("ld h,") {
        bytes.push(0x26);
        bytes.push(parse_u8(value.trim())?);
    } else if text == "ld l, a" {
        bytes.push(0x6F);
    } else if text == "ld l, c" {
        bytes.push(0x69);
    } else if text == "inc hl" {
        bytes.push(0x23);
    } else if text == "inc de" {
        bytes.push(0x13);
    } else if text == "dec bc" {
        bytes.push(0x0B);
    } else if text == "add hl, hl" {
        bytes.push(0x29);
    } else if text == "add hl, bc" {
        bytes.push(0x09);
    } else if text == "add hl, de" {
        bytes.push(0x19);
    } else if text == "add a, a" {
        bytes.push(0x87);
    } else if text == "sbc hl, bc" {
        bytes.extend([0xED, 0x42]);
    } else if text == "sbc hl, de" {
        bytes.extend([0xED, 0x52]);
    } else if text == "add a, b" {
        bytes.push(0x80);
    } else if text == "add a, c" {
        bytes.push(0x81);
    } else if text == "sub b" {
        bytes.push(0x90);
    } else if text == "sub c" {
        bytes.push(0x91);
    } else if text == "and b" {
        bytes.push(0xA0);
    } else if text == "and c" {
        bytes.push(0xA1);
    } else if text == "or b" {
        bytes.push(0xB0);
    } else if text == "or c" {
        bytes.push(0xB1);
    } else if text == "xor b" {
        bytes.push(0xA8);
    } else if text == "xor c" {
        bytes.push(0xA9);
    } else if text == "xor a" {
        bytes.push(0xAF);
    } else if let Some(value) = text.strip_prefix("xor ") {
        bytes.push(0xEE);
        bytes.push(parse_u8(value.trim())?);
    } else if text == "cp b" {
        bytes.push(0xB8);
    } else if text == "cp c" {
        bytes.push(0xB9);
    } else if text == "srl a" {
        bytes.extend([0xCB, 0x3F]);
    } else if text == "sra a" {
        bytes.extend([0xCB, 0x2F]);
    } else if text == "rl a" {
        bytes.extend([0xCB, 0x17]);
    } else if text == "rr a" {
        bytes.extend([0xCB, 0x1F]);
    } else if let Some(value) = text.strip_prefix("ld a,") {
        bytes.push(0x3E);
        bytes.push(parse_u8(value.trim())?);
    } else {
        return Err(Diagnostic::new(format!(
            "test assembler does not support instruction `{text}`"
        )));
    }
    Ok(())
}

fn relative_offset(pc: u32, target: u32) -> Result<u8, Diagnostic> {
    let next_pc = (pc + 2) & 0xFF_FFFF;
    let offset = target as i64 - next_pc as i64;
    if !(-128..=127).contains(&offset) {
        return Err(Diagnostic::new(format!(
            "relative jump target 0x{target:06X} is out of range from 0x{pc:06X}"
        )));
    }
    Ok((offset as i8) as u8)
}

fn parse_ld_reg8_reg8(text: &str) -> Option<(u8, u8)> {
    let (dst, src) = parse_ld_operands(text)?;
    Some((reg8_code(dst)?, reg8_code(src)?))
}

fn parse_ld_reg8_imm(text: &str) -> Result<Option<(u8, u8)>, Diagnostic> {
    let Some((dst, value)) = parse_ld_operands(text) else {
        return Ok(None);
    };
    let Some(dst) = reg8_code(dst) else {
        return Ok(None);
    };
    if reg8_code(value).is_some() || value.starts_with('(') {
        return Ok(None);
    }
    Ok(Some((dst, parse_u8(value)?)))
}

fn parse_ld_reg8_from_hl(text: &str) -> Option<u8> {
    let (dst, src) = parse_ld_operands(text)?;
    if src != "(hl)" {
        return None;
    }
    reg8_code(dst)
}

fn parse_ld_hl_from_reg8(text: &str) -> Option<u8> {
    let (dst, src) = parse_ld_operands(text)?;
    if dst != "(hl)" {
        return None;
    }
    reg8_code(src)
}

fn parse_ld_hl_imm(text: &str) -> Result<Option<u8>, Diagnostic> {
    let Some((dst, value)) = parse_ld_operands(text) else {
        return Ok(None);
    };
    if dst != "(hl)" || reg8_code(value).is_some() || value.starts_with('(') {
        return Ok(None);
    }
    Ok(Some(parse_u8(value)?))
}

fn parse_ld_operands(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("ld ")?;
    let (dst, src) = rest.split_once(',')?;
    Some((dst.trim(), src.trim()))
}

fn parse_ld_reg16_direct_load(text: &str) -> Option<(&str, &str)> {
    let (dst, src) = parse_ld_operands(text)?;
    if !matches!(dst, "bc" | "de") {
        return None;
    }
    Some((dst, parse_wrapped_indirect(src)?))
}

fn parse_ld_direct_reg16_store(text: &str) -> Option<(&str, &str)> {
    let (dst, src) = parse_ld_operands(text)?;
    if !matches!(src, "bc" | "de") {
        return None;
    }
    Some((parse_wrapped_indirect(dst)?, src))
}

fn parse_wrapped_indirect(text: &str) -> Option<&str> {
    text.strip_prefix('(')?.strip_suffix(')')
}

fn ld_reg16_direct_load_opcode(register: &str) -> u8 {
    match register {
        "bc" => 0x4B,
        "de" => 0x5B,
        _ => unreachable!("invalid direct-load register {register}"),
    }
}

fn ld_direct_reg16_store_opcode(register: &str) -> u8 {
    match register {
        "bc" => 0x43,
        "de" => 0x53,
        _ => unreachable!("invalid direct-store register {register}"),
    }
}

fn parse_block_operation(text: &str) -> Option<u8> {
    match text {
        "ldi" => Some(0xA0),
        "ldir" => Some(0xB0),
        "ldd" => Some(0xA8),
        "lddr" => Some(0xB8),
        "cpi" => Some(0xA1),
        "cpir" => Some(0xB1),
        "cpd" => Some(0xA9),
        "cpdr" => Some(0xB9),
        "ini" => Some(0xA2),
        "inir" => Some(0xB2),
        "ind" => Some(0xAA),
        "indr" => Some(0xBA),
        "outi" => Some(0xA3),
        "otir" => Some(0xB3),
        "outd" => Some(0xAB),
        "otdr" => Some(0xBB),
        _ => None,
    }
}

fn parse_mlt_reg16(text: &str) -> Option<u8> {
    let register = text.strip_prefix("mlt ")?;
    match register.trim() {
        "bc" => Some(0x4C),
        "de" => Some(0x5C),
        "hl" => Some(0x6C),
        "sp" => Some(0x7C),
        _ => None,
    }
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

fn parse_inc_dec_reg8(text: &str) -> Option<(bool, u8)> {
    if let Some(register) = text.strip_prefix("inc ") {
        return Some((true, reg8_code(register.trim())?));
    }
    if let Some(register) = text.strip_prefix("dec ") {
        return Some((false, reg8_code(register.trim())?));
    }
    None
}

fn inc_dec_reg8_opcode(inc: bool, register: u8) -> u8 {
    let base = if inc { 0x04 } else { 0x05 };
    base + register * 8
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

fn parse_accumulator_alu_reg8(text: &str) -> Option<(AccumulatorAluOp, u8)> {
    if let Some(src) = text.strip_prefix("add a,") {
        return Some((AccumulatorAluOp::Add, reg8_code(src.trim())?));
    }
    if let Some(src) = text.strip_prefix("adc a,") {
        return Some((AccumulatorAluOp::Adc, reg8_code(src.trim())?));
    }
    if let Some(src) = text.strip_prefix("sbc a,") {
        return Some((AccumulatorAluOp::Sbc, reg8_code(src.trim())?));
    }
    for (prefix, op) in [
        ("sub ", AccumulatorAluOp::Sub),
        ("and ", AccumulatorAluOp::And),
        ("or ", AccumulatorAluOp::Or),
        ("xor ", AccumulatorAluOp::Xor),
        ("cp ", AccumulatorAluOp::Cp),
    ] {
        if let Some(src) = text.strip_prefix(prefix) {
            return Some((op, reg8_code(src.trim())?));
        }
    }
    None
}

fn parse_accumulator_alu_imm(text: &str) -> Result<Option<(AccumulatorAluOp, u8)>, Diagnostic> {
    if let Some(src) = text.strip_prefix("add a,") {
        let src = src.trim();
        if reg8_code(src).is_some() {
            return Ok(None);
        }
        return Ok(Some((AccumulatorAluOp::Add, parse_u8(src)?)));
    }
    if let Some(src) = text.strip_prefix("adc a,") {
        let src = src.trim();
        if reg8_code(src).is_some() {
            return Ok(None);
        }
        return Ok(Some((AccumulatorAluOp::Adc, parse_u8(src)?)));
    }
    if let Some(src) = text.strip_prefix("sbc a,") {
        let src = src.trim();
        if reg8_code(src).is_some() {
            return Ok(None);
        }
        return Ok(Some((AccumulatorAluOp::Sbc, parse_u8(src)?)));
    }
    for (prefix, op) in [
        ("sub ", AccumulatorAluOp::Sub),
        ("and ", AccumulatorAluOp::And),
        ("or ", AccumulatorAluOp::Or),
        ("xor ", AccumulatorAluOp::Xor),
        ("cp ", AccumulatorAluOp::Cp),
    ] {
        if let Some(src) = text.strip_prefix(prefix) {
            let src = src.trim();
            if reg8_code(src).is_some() {
                return Ok(None);
            }
            return Ok(Some((op, parse_u8(src)?)));
        }
    }
    Ok(None)
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IndexRegister {
    Ix,
    Iy,
}

impl IndexRegister {
    fn prefix(self) -> u8 {
        match self {
            IndexRegister::Ix => 0xDD,
            IndexRegister::Iy => 0xFD,
        }
    }
}

fn is_index_byte_load_or_store(text: &str) -> bool {
    parse_index_byte_load(text).is_ok_and(|offset| offset.is_some())
        || parse_index_byte_store(text).is_ok_and(|offset| offset.is_some())
}

fn parse_index_byte_load(text: &str) -> Result<Option<(IndexRegister, u8)>, Diagnostic> {
    for (prefix, register) in [
        ("ld a, (ix", IndexRegister::Ix),
        ("ld a, (iy", IndexRegister::Iy),
    ] {
        let Some(rest) = text.strip_prefix(prefix) else {
            continue;
        };
        return parse_index_offset(rest).map(|offset| Some((register, offset)));
    }
    Ok(None)
}

fn parse_index_byte_store(text: &str) -> Result<Option<(IndexRegister, u8)>, Diagnostic> {
    for (prefix, register) in [("ld (ix", IndexRegister::Ix), ("ld (iy", IndexRegister::Iy)] {
        let Some(rest) = text.strip_prefix(prefix) else {
            continue;
        };
        let Some(rest) = rest.strip_suffix("), a") else {
            return Ok(None);
        };
        return parse_index_offset(rest).map(|offset| Some((register, offset)));
    }
    Ok(None)
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

fn parse_addr(text: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u32, Diagnostic> {
    if text == "$" {
        return Ok(pc & 0xFF_FFFF);
    }
    if let Some(addr) = labels.get(text).copied() {
        return Ok(addr);
    }
    match parse_number(text) {
        Ok(value) if value <= Address24::MAX => Ok(value),
        Ok(_) => Err(Diagnostic::new(format!(
            "address operand `{text}` is outside the 24-bit address space"
        ))),
        Err(_) if looks_like_label_ref(text) => {
            Err(Diagnostic::new(format!("unknown assembly label `{text}`")))
        }
        Err(error) => Err(error),
    }
}

fn looks_like_label_ref(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '.' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '.' || ch.is_ascii_alphanumeric())
}

fn parse_u8(text: &str) -> Result<u8, Diagnostic> {
    let value = parse_number(text)?;
    if value > 0xFF {
        return Err(Diagnostic::new(format!("value {text} is outside u8 range")));
    }
    Ok(value as u8)
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

fn push24(bytes: &mut Vec<u8>, value: u32) {
    bytes.push(value as u8);
    bytes.push((value >> 8) as u8);
    bytes.push((value >> 16) as u8);
}

struct TestMachine {
    memory: HashMap<u32, u8>,
    ports: [u8; 256],
    cycles: Cell<i64>,
    halted: bool,
    result_code: u8,
    debug_output: Vec<u8>,
}

impl TestMachine {
    fn new() -> Self {
        Self {
            memory: HashMap::new(),
            ports: [0; 256],
            cycles: Cell::new(0),
            halted: false,
            result_code: 0,
            debug_output: Vec::new(),
        }
    }
}

impl Machine for TestMachine {
    fn peek(&self, address: u32) -> u8 {
        self.memory
            .get(&(address & 0xFF_FFFF))
            .copied()
            .unwrap_or(0)
    }

    fn poke(&mut self, address: u32, value: u8) {
        self.memory.insert(address & 0xFF_FFFF, value);
    }

    fn use_cycles(&self, cycles: i32) {
        self.cycles
            .set(self.cycles.get().wrapping_add(cycles as i64));
    }

    fn port_in(&mut self, address: u16) -> u8 {
        self.ports[address as usize & 0xFF]
    }

    fn port_out(&mut self, address: u16, value: u8) {
        let port = address as usize & 0xFF;
        self.ports[port] = value;
        if port == 0x0C {
            self.debug_output.push(value);
        }
        if port == 0x0D {
            self.result_code = value;
        }
        if port == 0x0E && value == 1 {
            self.halted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{asm::emit_ez80_assembly, parser::parse_program};

    use super::*;

    #[test]
    fn runs_emitted_test_pass_on_ez80_vm() {
        let program = parse_program(Path::new("game.ezra"), "fn main() { test.pass() }").unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
        assert_eq!(run.failure, None);
    }

    #[test]
    fn reports_timeout_when_program_does_not_halt() {
        let run = run_assembly_test("spin:\n    jp spin\n", 3).unwrap();

        assert!(!run.halted);
        assert_eq!(run.instructions, 3);
        assert_eq!(run.failure, Some(TestRunFailure::Timeout));
    }

    #[test]
    fn runs_current_address_jump_on_ez80_vm() {
        let run = run_assembly_test("jp $\n", 3).unwrap();

        assert!(!run.halted);
        assert_eq!(run.instructions, 3);
        assert_eq!(run.failure, Some(TestRunFailure::Timeout));
    }

    #[test]
    fn reports_execution_outside_mapped_memory() {
        let run = run_assembly_test("jp 020000h\n", 10).unwrap();

        assert!(!run.halted);
        assert_eq!(run.instructions, 1);
        assert_eq!(
            run.failure,
            Some(TestRunFailure::ExecutionOutsideMappedMemory { pc: 0x020000 })
        );
    }

    #[test]
    fn initializes_stack_pointer_to_default_stack_top() {
        let asm = r#"
            call leaves_return_address
            ld a, (0EFFFFDh)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        leaves_return_address:
            ret
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0x04);
    }

    #[test]
    fn run_options_set_initial_stack_top() {
        let asm = r#"
            call leaves_return_address
            ld a, (0402FDh)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        leaves_return_address:
            ret
        "#;
        let run = run_assembly_test_with_options(
            asm,
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x040300,
            },
        )
        .unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0x04);
    }

    #[test]
    fn rejects_stack_top_outside_address_space() {
        let error = run_assembly_test_with_options(
            "ret\n",
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x01_000000,
            },
        )
        .unwrap_err();

        assert_eq!(
            error.message,
            "test stack top 0x1000000 is outside the 24-bit address space"
        );
    }

    #[test]
    fn reports_stack_overflow_into_non_stack_memory() {
        let asm = r#"
            ld sp, 030400h
            ld hl, 012345h
            push hl
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test_with_options(
            asm,
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x040400,
            },
        )
        .unwrap();

        assert!(!run.halted);
        assert_eq!(
            run.failure,
            Some(TestRunFailure::StackOverflow { sp: 0x0303FD })
        );
    }

    #[test]
    fn assembles_interrupt_enable_and_disable_instructions() {
        let bytes = assemble_ez80_subset_at("di\nei\nret\n", EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0xF3, 0xFB, 0xC9]);
    }

    #[test]
    fn assembles_nop_instruction() {
        let bytes = assemble_ez80_subset_at("nop\nret\n", EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0x00, 0xC9]);
    }

    #[test]
    fn runs_inline_asm_nop_on_ez80_vm() {
        let source = r#"
            fn main() {
                asm volatile {
                    "nop"
                }
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 1_000).unwrap();

        assert!(asm.contains("    nop"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn assembles_arithmetic_shift_right_accumulator() {
        let bytes = assemble_ez80_subset_at("sra a\nret\n", EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0xCB, 0x2F, 0xC9]);
    }

    #[test]
    fn assembles_8_bit_register_loads() {
        let asm = r#"
            ld b, 12h
            ld c, 34h
            ld d, 56h
            ld e, 78h
            ld h, 9Ah
            ld l, 0BCh
            ld a, 0DEh
            ld e, a
            ld a, e
            ld l, b
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0x06, 0x12, 0x0E, 0x34, 0x16, 0x56, 0x1E, 0x78, 0x26, 0x9A, 0x2E, 0xBC, 0x3E, 0xDE,
                0x5F, 0x7B, 0x68,
            ]
        );
    }

    #[test]
    fn runs_8_bit_register_loads_on_ez80_vm() {
        let asm = r#"
            ld e, 00h
            ld a, 43h
            ld e, a
            ld a, e
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
        assert_eq!(run.debug_output, b"C");
    }

    #[test]
    fn assembles_bc_de_indirect_accumulator_loads_and_stores() {
        let asm = r#"
            ld a, (bc)
            ld (bc), a
            ld a, (de)
            ld (de), a
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0x0A, 0x02, 0x1A, 0x12]);
    }

    #[test]
    fn runs_bc_de_indirect_accumulator_loads_and_stores_on_ez80_vm() {
        let asm = r#"
            ld bc, 040100h
            ld de, 040101h
            ld a, 42h
            ld (bc), a
            ld a, 44h
            ld (de), a
            ld a, (bc)
            out0 (0Ch), a
            ld a, (de)
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
        assert_eq!(run.debug_output, b"BD");
    }

    #[test]
    fn assembles_bc_de_direct_memory_loads_and_stores() {
        let asm = r#"
            ld bc, (040100h)
            ld de, (040103h)
            ld (040106h), bc
            ld (040109h), de
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xED, 0x4B, 0x00, 0x01, 0x04, 0xED, 0x5B, 0x03, 0x01, 0x04, 0xED, 0x43, 0x06,
                0x01, 0x04, 0xED, 0x53, 0x09, 0x01, 0x04,
            ]
        );
    }

    #[test]
    fn runs_bc_de_direct_memory_loads_and_stores_on_ez80_vm() {
        let asm = r#"
            ld bc, 004244h
            ld (040100h), bc
            ld de, (040100h)
            ld a, d
            out0 (0Ch), a
            ld a, e
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
        assert_eq!(run.debug_output, b"BD");
    }

    #[test]
    fn assembles_hl_indirect_8_bit_loads_and_stores() {
        let asm = r#"
            ld b, (hl)
            ld c, (hl)
            ld d, (hl)
            ld e, (hl)
            ld h, (hl)
            ld l, (hl)
            ld a, (hl)
            ld (hl), b
            ld (hl), c
            ld (hl), d
            ld (hl), e
            ld (hl), h
            ld (hl), l
            ld (hl), a
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0x46, 0x4E, 0x56, 0x5E, 0x66, 0x6E, 0x7E, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x77,
            ]
        );
    }

    #[test]
    fn runs_hl_indirect_8_bit_loads_and_stores_on_ez80_vm() {
        let asm = r#"
            ld hl, 040100h
            ld a, 41h
            ld (hl), a
            ld b, (hl)
            inc hl
            ld (hl), b
            ld e, (hl)
            ld a, e
            add a, 02h
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
        assert_eq!(run.debug_output, b"C");
    }

    #[test]
    fn assembles_hl_indirect_immediate_store() {
        let bytes = assemble_ez80_subset_at("ld (hl), 43h\n", EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0x36, 0x43]);
    }

    #[test]
    fn runs_hl_indirect_immediate_store_on_ez80_vm() {
        let asm = r#"
            ld hl, 040100h
            ld (hl), 43h
            ld a, (hl)
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
        assert_eq!(run.debug_output, b"C");
    }

    #[test]
    fn assembles_8_bit_register_inc_and_dec() {
        let asm = r#"
            inc b
            inc c
            inc d
            inc e
            inc h
            inc l
            inc a
            dec b
            dec c
            dec d
            dec e
            dec h
            dec l
            dec a
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0x04, 0x0C, 0x14, 0x1C, 0x24, 0x2C, 0x3C, 0x05, 0x0D, 0x15, 0x1D, 0x25, 0x2D, 0x3D,
            ]
        );
    }

    #[test]
    fn runs_8_bit_register_inc_and_dec_on_ez80_vm() {
        let asm = r#"
            ld e, 42h
            inc e
            ld a, e
            dec a
            inc a
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
        assert_eq!(run.debug_output, b"C");
    }

    #[test]
    fn assembles_8_bit_accumulator_alu_register_forms() {
        let asm = r#"
            add a, b
            add a, e
            adc a, c
            adc a, h
            sub d
            sub l
            sbc a, b
            sbc a, e
            and h
            or e
            xor l
            cp d
            cp a
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0x80, 0x83, 0x89, 0x8C, 0x92, 0x95, 0x98, 0x9B, 0xA4, 0xB3, 0xAD, 0xBA, 0xBF,
            ]
        );
    }

    #[test]
    fn assembles_ez80_mlt_register_forms() {
        let bytes = assemble_ez80_subset_at(
            r#"
            mlt bc
            mlt de
            mlt hl
            mlt sp
            "#,
            EZRA_LOAD_ADDR.get(),
        )
        .unwrap();

        assert_eq!(bytes, [0xED, 0x4C, 0xED, 0x5C, 0xED, 0x6C, 0xED, 0x7C]);
    }

    #[test]
    fn runs_ez80_mlt_register_form_on_vm() {
        let asm = r#"
            ld b, 11h
            ld c, 0Fh
            mlt bc
            ld a, c
            cp 0FFh
            jp nz, fail
            ld a, b
            cp 00h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 02h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }

    #[test]
    fn assembles_ez80_block_transfer_instructions() {
        let bytes = assemble_ez80_subset_at(
            r#"
            ldi
            ldir
            ldd
            lddr
            "#,
            EZRA_LOAD_ADDR.get(),
        )
        .unwrap();

        assert_eq!(bytes, [0xED, 0xA0, 0xED, 0xB0, 0xED, 0xA8, 0xED, 0xB8]);
    }

    #[test]
    fn assembles_ez80_block_compare_instructions() {
        let bytes = assemble_ez80_subset_at(
            r#"
            cpi
            cpir
            cpd
            cpdr
            "#,
            EZRA_LOAD_ADDR.get(),
        )
        .unwrap();

        assert_eq!(bytes, [0xED, 0xA1, 0xED, 0xB1, 0xED, 0xA9, 0xED, 0xB9]);
    }

    #[test]
    fn runs_ez80_ldir_on_vm() {
        let asm = r#"
            ld a, 41h
            ld (040300h), a
            ld a, 42h
            ld (040301h), a
            ld a, 43h
            ld (040302h), a
            ld hl, 040300h
            ld de, 040310h
            ld bc, 000003h
            ldir
            ld a, (040310h)
            cp 41h
            jp nz, fail
            ld a, (040311h)
            cp 42h
            jp nz, fail
            ld a, (040312h)
            cp 43h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 02h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 200).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }

    #[test]
    fn runs_ez80_cpir_on_vm() {
        let asm = r#"
            ld a, 11h
            ld (040300h), a
            ld a, 42h
            ld (040301h), a
            ld a, 33h
            ld (040302h), a
            ld a, 42h
            ld hl, 040300h
            ld bc, 000003h
            cpir
            jp nz, fail
            ld a, c
            cp 01h
            jp nz, fail
            ld (040310h), hl
            ld a, (040310h)
            cp 02h
            jp nz, fail
            ld a, (040311h)
            cp 03h
            jp nz, fail
            ld a, (040312h)
            cp 04h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 02h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 300).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }

    #[test]
    fn assembles_ez80_block_io_instructions() {
        let bytes = assemble_ez80_subset_at(
            r#"
            ini
            inir
            ind
            indr
            outi
            otir
            outd
            otdr
            "#,
            EZRA_LOAD_ADDR.get(),
        )
        .unwrap();

        assert_eq!(
            bytes,
            [
                0xED, 0xA2, 0xED, 0xB2, 0xED, 0xAA, 0xED, 0xBA, 0xED, 0xA3, 0xED, 0xB3, 0xED, 0xAB,
                0xED, 0xBB,
            ]
        );
    }

    #[test]
    fn runs_ez80_otir_on_vm() {
        let asm = r#"
            ld a, 11h
            ld (040320h), a
            ld a, 42h
            ld (040321h), a
            ld hl, 040320h
            ld bc, 000220h
            otir
            ld a, c
            cp 20h
            jp nz, fail
            ld a, b
            cp 00h
            jp nz, fail
            ld (040330h), hl
            ld a, (040330h)
            cp 22h
            jp nz, fail
            ld a, (040331h)
            cp 03h
            jp nz, fail
            ld a, (040332h)
            cp 04h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 02h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 400).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
        assert_eq!(run.ports[0x20], 0x42);
    }

    #[test]
    fn runs_8_bit_accumulator_alu_register_forms_on_ez80_vm() {
        let asm = r#"
            ld a, 40h
            ld e, 04h
            add a, e
            cp 45h
            ld e, 00h
            adc a, e
            cp 46h
            ld e, 01h
            sbc a, e
            ld d, 01h
            sub d
            ld l, 03h
            or l
            ld h, 7Fh
            and h
            ld e, 00h
            xor e
            cp e
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
        assert_eq!(run.debug_output, b"C");
    }

    #[test]
    fn assembles_8_bit_accumulator_alu_immediate_forms() {
        let asm = r#"
            add a, 01h
            adc a, 02h
            sub 02h
            sbc a, 03h
            and 03h
            xor 04h
            or 05h
            cp 06h
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xC6, 0x01, 0xCE, 0x02, 0xD6, 0x02, 0xDE, 0x03, 0xE6, 0x03, 0xEE, 0x04, 0xF6,
                0x05, 0xFE, 0x06,
            ]
        );
    }

    #[test]
    fn runs_8_bit_accumulator_alu_immediate_forms_on_ez80_vm() {
        let asm = r#"
            ld a, 40h
            add a, 04h
            cp 45h
            adc a, 00h
            cp 46h
            sbc a, 01h
            sub 01h
            or 03h
            and 7Fh
            xor 00h
            cp 43h
            jp z, ok
            ld a, 01h
            out0 (0Dh), a
            out0 (0Eh), a
        ok:
            out0 (0Ch), a
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
        assert_eq!(run.debug_output, b"C");
    }

    #[test]
    fn assembles_relative_jumps() {
        let asm = r#"
            jr next
            ret
        next:
            jr z, done
            jr nz, done
            jr c, done
            jr nc, done
        done:
            jr next
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0x18, 0x01, 0xC9, 0x28, 0x06, 0x20, 0x04, 0x38, 0x02, 0x30, 0x00, 0x18, 0xF6,
            ]
        );
    }

    #[test]
    fn assembles_current_address_jumps() {
        let asm = r#"
            jp $
            jr $
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0xC3, 0x00, 0x00, 0x01, 0x18, 0xFE]);
    }

    #[test]
    fn rejects_duplicate_assembly_labels() {
        let asm = r#"
        again:
            jp again
        again:
            ret
        "#;
        let error = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap_err();

        assert_eq!(error.message, "duplicate assembly label `again`");
    }

    #[test]
    fn rejects_unknown_assembly_labels() {
        let error =
            assemble_ez80_subset_at("jp missing_label\n", EZRA_LOAD_ADDR.get()).unwrap_err();

        assert_eq!(error.message, "unknown assembly label `missing_label`");
    }

    #[test]
    fn rejects_invalid_numeric_jump_operands() {
        let error = assemble_ez80_subset_at("jp 0xBADHEX\n", EZRA_LOAD_ADDR.get()).unwrap_err();

        assert_eq!(error.message, "invalid numeric operand `0xBADHEX`");
    }

    #[test]
    fn rejects_address_operands_outside_address_space() {
        let error = assemble_ez80_subset_at("jp 0x1000000\n", EZRA_LOAD_ADDR.get()).unwrap_err();

        assert_eq!(
            error.message,
            "address operand `0x1000000` is outside the 24-bit address space"
        );
    }

    #[test]
    fn rejects_relative_jumps_outside_signed_byte_range() {
        let padding = "ret\n".repeat(128);
        let asm = format!("jr far\n{padding}far:\nret\n");
        let error = assemble_ez80_subset_at(&asm, EZRA_LOAD_ADDR.get()).unwrap_err();

        assert_eq!(
            error.message,
            "relative jump target 0x010082 is out of range from 0x010000"
        );
    }

    #[test]
    fn runs_relative_jump_loop_on_ez80_vm() {
        let asm = r#"
            ld a, 03h
            ld b, a
        loop:
            dec b
            jr z, done
            jr loop
        done:
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }

    #[test]
    fn assembles_djnz_relative_loop() {
        let asm = r#"
            ld a, 03h
            ld b, a
        loop:
            djnz loop
            ret
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0x3E, 0x03, 0x47, 0x10, 0xFE, 0xC9]);
    }

    #[test]
    fn runs_djnz_loop_on_ez80_vm() {
        let asm = r#"
            ld a, 03h
            ld b, a
        loop:
            djnz loop
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }

    #[test]
    fn runs_conditional_returns_on_ez80_vm() {
        let asm = r#"
            ld a, 01h
            or a
            call check_nz

            ld b, a
            cp b
            call check_z

            ld a, 01h
            or a
            call check_nc

            ld b, 01h
            ld a, 00h
            cp b
            call check_c

            ld a, 00h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        check_nz:
            ret nz
            ld a, 10h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        check_z:
            ret z
            ld a, 11h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        check_nc:
            ret nc
            ld a, 12h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        check_c:
            ret c
            ld a, 13h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert!(bytes.contains(&0xC0));
        assert!(bytes.contains(&0xC8));
        assert!(bytes.contains(&0xD0));
        assert!(bytes.contains(&0xD8));

        let run = run_assembly_test(asm, 200).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }

    #[test]
    fn compare_carry_jump_sequence_matches_emitter_assumption() {
        let asm = r#"
            ld sp, 0F00000h
            ld a, 00h
            ld b, a
            ld a, 04h
            ld c, a
            ld a, b
            cp c
            jp c, yes
            ld a, 09h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        yes:
            ld a, 00h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }

    #[test]
    fn run_options_seed_input_ports() {
        let asm = r#"
            in0 a, (01h)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test_with_options(
            asm,
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: vec![(0x01, 0x2A)],
                initial_memory: Vec::new(),
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
        .unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0x2A);
    }

    #[test]
    fn run_options_seed_memory() {
        let asm = r#"
            ld a, (040123h)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test_with_options(
            asm,
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: vec![(0x040123, 0x6C)],
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
        .unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0x6C);
    }

    #[test]
    fn rejects_initial_memory_outside_address_space() {
        let error = run_assembly_test_with_options(
            "ret\n",
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: vec![(0x01_000000, 0x6C)],
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
        .unwrap_err();

        assert_eq!(
            error.message,
            "test memory address 0x1000000 is outside the 24-bit address space"
        );
    }

    #[test]
    fn rejects_assembly_base_outside_address_space() {
        let error = assemble_ez80_subset_at("ret\n", 0x01_000000).unwrap_err();

        assert_eq!(
            error.message,
            "assembly base address 0x1000000 is outside the 24-bit address space"
        );
    }

    #[test]
    fn rejects_test_program_that_exceeds_address_space() {
        let error = run_assembly_test_with_options_at(
            "nop\nnop\n",
            &TestRunOptions {
                instruction_budget: 100,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: EZRA_STACK_TOP.get(),
            },
            0xFF_FFFF,
        )
        .unwrap_err();

        assert_eq!(
            error.message,
            "test program at 0xFFFFFF with length 0x2 exceeds the 24-bit address space"
        );
    }

    #[test]
    fn runs_ix_displacement_loads_and_stores() {
        let asm = r#"
            ld sp, 0F00000h
            ld ix, 040200h
            ld a, 2Ah
            ld (ix+3), a
            ld a, 00h
            ld a, (ix+3)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0x2A);
    }

    #[test]
    fn runs_iy_displacement_loads_and_stores() {
        let asm = r#"
            ld sp, 0F00000h
            ld iy, 040200h
            ld a, 35h
            ld (iy+3), a
            ld a, 00h
            ld a, (iy+3)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert!(
            bytes
                .windows(5)
                .any(|window| window == [0xFD, 0x21, 0x00, 0x02, 0x04])
        );
        assert!(bytes.windows(3).any(|window| window == [0xFD, 0x77, 0x03]));
        assert!(bytes.windows(3).any(|window| window == [0xFD, 0x7E, 0x03]));

        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0x35);
    }

    #[test]
    fn runs_ix_push_pop_and_sp_add() {
        let asm = r#"
            ld sp, 040400h
            ld ix, 000000h
            add ix, sp
            ld a, 11h
            ld (ix+1), a
            ld b, a
            ld a, (040401h)
            cp b
            jp z, sp_ok
            ld a, 0EEh
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        sp_ok:
            ld ix, 040220h
            push ix
            ld ix, 040240h
            pop ix
            ld a, 07h
            ld (ix+0), a
            ld a, (040220h)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let run = run_assembly_test_with_options(
            asm,
            &TestRunOptions {
                instruction_budget: 200,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x040400,
            },
        )
        .unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 7);
    }

    #[test]
    fn runs_iy_push_pop_and_sp_add() {
        let asm = r#"
            ld sp, 040400h
            ld iy, 000000h
            add iy, sp
            ld a, 12h
            ld (iy+1), a
            ld b, a
            ld a, (040401h)
            cp b
            jp z, sp_ok
            ld a, 0EEh
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        sp_ok:
            ld iy, 040220h
            push iy
            ld iy, 040240h
            pop iy
            ld a, 09h
            ld (iy+0), a
            ld a, (040220h)
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert!(bytes.windows(2).any(|window| window == [0xFD, 0x39]));
        assert!(bytes.windows(2).any(|window| window == [0xFD, 0xE5]));
        assert!(bytes.windows(2).any(|window| window == [0xFD, 0xE1]));

        let run = run_assembly_test_with_options(
            asm,
            &TestRunOptions {
                instruction_budget: 200,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0x040400,
            },
        )
        .unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 9);
    }
}
