use std::{
    cell::Cell,
    collections::{BTreeMap, HashMap},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
};

use ez80::{Cpu, CpuMode, Machine, Reg8, Reg16};

use crate::asm::ez80 as asm_meta;
use crate::diagnostic::{Diagnostic, SourceLocation};
use crate::target::{Address24, AssemblerCpu, CpuFamily, EZRA_LOAD_ADDR, EZRA_STACK_TOP};

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AssemblerSourceOptions {
    pub source_path: Option<PathBuf>,
    pub symbols: Vec<AssemblySymbol>,
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
    run_assembly_test_with_cpu_options_at(CpuFamily::Ez80, assembly, options, base_addr)
}

pub fn run_assembly_test_with_cpu_options_at(
    cpu_family: CpuFamily,
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

    let code = assemble_subset_at(cpu_family, assembly, base_addr)?;
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

    let mut cpu = Cpu::new_for_mode(cpu_mode_for_family(cpu_family));
    cpu.state.reg.adl = cpu_family == CpuFamily::Ez80;
    cpu.state.set_pc(base_addr);
    if cpu_family == CpuFamily::Z80 {
        cpu.state.reg.set16(Reg16::SP, options.stack_top as u16);
    } else {
        cpu.state.reg.set24(Reg16::SP, options.stack_top);
    }
    if std::env::var_os("EZRA_TRACE_VM").is_some() {
        cpu.set_trace(true);
    }

    for instruction in 0..options.instruction_budget {
        let pc = cpu.state.pc();
        if cpu_family == CpuFamily::Z80 && handle_cpm_bdos_call(&mut cpu, &mut machine)? {
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
            continue;
        }
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

fn cpu_mode_for_family(cpu: CpuFamily) -> CpuMode {
    match cpu {
        CpuFamily::Ez80 => CpuMode::EZ80,
        CpuFamily::Z80 => CpuMode::Z80,
        CpuFamily::Z80N => CpuMode::Z80N,
        CpuFamily::Z180 => CpuMode::Z180,
        CpuFamily::I8080 => CpuMode::I8080,
        CpuFamily::I8085 => CpuMode::I8085,
        CpuFamily::M68k => CpuMode::Z80,
    }
}

fn handle_cpm_bdos_call(cpu: &mut Cpu, machine: &mut TestMachine) -> Result<bool, Diagnostic> {
    if cpu.state.pc() != 0x0005 {
        return Ok(false);
    }

    match cpu.state.reg.get8(Reg8::C) {
        0 => machine.halted = true,
        2 => machine.debug_output.push(cpu.state.reg.get8(Reg8::E)),
        9 => {
            let mut address = cpu.state.reg.get16(Reg16::DE) as u32;
            for _ in 0..0x1_0000 {
                let byte = machine.peek(address);
                if byte == b'$' {
                    break;
                }
                machine.debug_output.push(byte);
                address = address.wrapping_add(1) & 0xFFFF;
            }
        }
        function => {
            return Err(Diagnostic::new(format!(
                "CP/M BDOS function {function} is not implemented by the test runner"
            )));
        }
    }

    let sp = cpu.state.reg.get16(Reg16::SP) as u32;
    let return_addr = machine.peek(sp) as u32 | ((machine.peek(sp.wrapping_add(1)) as u32) << 8);
    cpu.state.reg.set16(Reg16::SP, sp.wrapping_add(2) as u16);
    cpu.state.set_pc(return_addr);
    Ok(true)
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
    assemble_subset_with_symbols_at(AssemblerCpu::Ez80, assembly, base_addr)
}

pub fn assemble_subset_at(
    cpu_family: CpuFamily,
    assembly: &str,
    base_addr: u32,
) -> Result<Vec<u8>, Diagnostic> {
    Ok(assemble_subset_with_symbols_at(AssemblerCpu::from(cpu_family), assembly, base_addr)?.bytes)
}

pub fn assemble_subset_with_symbols_at(
    cpu: AssemblerCpu,
    assembly: &str,
    base_addr: u32,
) -> Result<AssembledProgram, Diagnostic> {
    assemble_subset_with_options_at(cpu, assembly, base_addr, &AssemblerSourceOptions::default())
}

pub fn assemble_subset_with_options_at(
    cpu: AssemblerCpu,
    assembly: &str,
    base_addr: u32,
    options: &AssemblerSourceOptions,
) -> Result<AssembledProgram, Diagnostic> {
    if base_addr > Address24::MAX {
        return Err(Diagnostic::new(format!(
            "assembly base address 0x{base_addr:X} is outside the 24-bit address space"
        )));
    }
    let instructions = assembly
        .lines()
        .enumerate()
        .filter_map(|(index, line)| parse_line(index + 1, line))
        .collect::<Vec<_>>();
    let mut labels = BTreeMap::new();
    for symbol in &options.symbols {
        labels.insert(symbol.name.clone(), symbol.addr);
    }
    let mut pc = base_addr & 0xFF_FFFF;

    for instruction in &instructions {
        match &instruction.kind {
            AsmLine::Label(name) => {
                if pc > Address24::MAX {
                    return Err(line_diagnostic(
                        instruction,
                        options,
                        format!(
                            "assembly label `{name}` address 0x{pc:X} is outside the 24-bit address space"
                        ),
                    ));
                }
                if labels.insert(name.clone(), pc).is_some() {
                    return Err(line_diagnostic(
                        instruction,
                        options,
                        format!("duplicate assembly label `{name}`"),
                    ));
                }
            }
            AsmLine::Equ { name, expr } => {
                let value = eval_expr(expr, &labels.clone().into_iter().collect(), pc).map_err(
                    |error| error.with_location_if_missing(line_location(instruction, options)),
                )?;
                if labels.insert(name.clone(), value).is_some() {
                    return Err(line_diagnostic(
                        instruction,
                        options,
                        format!("duplicate assembly symbol `{name}`"),
                    ));
                }
            }
            AsmLine::Section(_) => {}
            AsmLine::Org(expr) => {
                pc = eval_expr(expr, &labels.clone().into_iter().collect(), pc).map_err(
                    |error| error.with_location_if_missing(line_location(instruction, options)),
                )?;
            }
            AsmLine::Data(values) => {
                pc = checked_assembly_pc_advance(pc, data_len(values) as u32).map_err(|error| {
                    error.with_location_if_missing(line_location(instruction, options))
                })?;
            }
            AsmLine::Instruction(text) => {
                let len = instruction_len(cpu, text).map_err(|error| {
                    error.with_location_if_missing(line_location(instruction, options))
                })?;
                pc = checked_assembly_pc_advance(pc, len as u32).map_err(|error| {
                    error.with_location_if_missing(line_location(instruction, options))
                })?;
            }
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
        match &instruction.kind {
            AsmLine::Label(_) | AsmLine::Equ { .. } | AsmLine::Section(_) => {}
            AsmLine::Org(expr) => {
                let new_pc = eval_expr(expr, &labels, pc).map_err(|error| {
                    error.with_location_if_missing(line_location(&instruction, options))
                })?;
                append_org_padding(&mut bytes, pc, new_pc).map_err(|error| {
                    error.with_location_if_missing(line_location(&instruction, options))
                })?;
                pc = new_pc;
            }
            AsmLine::Data(values) => {
                emit_data(values, &labels, pc, &mut bytes).map_err(|error| {
                    error.with_location_if_missing(line_location(&instruction, options))
                })?;
                pc = checked_assembly_pc_advance(pc, data_len(values) as u32).map_err(|error| {
                    error.with_location_if_missing(line_location(&instruction, options))
                })?;
            }
            AsmLine::Instruction(text) => {
                emit_instruction(cpu, text, &labels, pc, &mut bytes).map_err(|error| {
                    error.with_location_if_missing(line_location(&instruction, options))
                })?;
                let len = instruction_len(cpu, text).map_err(|error| {
                    error.with_location_if_missing(line_location(&instruction, options))
                })?;
                pc = checked_assembly_pc_advance(pc, len as u32).map_err(|error| {
                    error.with_location_if_missing(line_location(&instruction, options))
                })?;
            }
        }
    }
    Ok(AssembledProgram { bytes, symbols })
}

fn checked_assembly_pc_advance(pc: u32, len: u32) -> Result<u32, Diagnostic> {
    let end = pc
        .checked_add(len)
        .ok_or_else(|| Diagnostic::new("assembly exceeds the 24-bit address space"))?;
    if end > Address24::MAX + 1 {
        return Err(Diagnostic::new(format!(
            "assembly instruction at 0x{pc:06X} with length 0x{len:X} exceeds the 24-bit address space"
        )));
    }
    Ok(end)
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
    Equ { name: String, expr: String },
    Section(String),
    Org(String),
    Data(Vec<DataValue>),
    Instruction(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DataValue {
    Byte(String),
    Word(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocatedAsmLine {
    line: usize,
    column: usize,
    kind: AsmLine,
}

fn parse_line(line_number: usize, line: &str) -> Option<LocatedAsmLine> {
    let line = line.split(';').next().unwrap_or("");
    let column = line
        .chars()
        .position(|ch| !ch.is_whitespace())
        .map(|index| index + 1)
        .unwrap_or(1);
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let kind = if let Some(section) = line.strip_prefix("section ") {
        AsmLine::Section(section.trim().to_owned())
    } else if let Some(expr) = line.strip_prefix("org ") {
        AsmLine::Org(expr.trim().to_owned())
    } else if let Some(values) = line
        .strip_prefix("db ")
        .or_else(|| line.strip_prefix("byte "))
    {
        AsmLine::Data(parse_data_values(values, DataWidth::Byte))
    } else if let Some(values) = line
        .strip_prefix("dw ")
        .or_else(|| line.strip_prefix("word "))
    {
        AsmLine::Data(parse_data_values(values, DataWidth::Word))
    } else if let Some((name, expr)) = parse_equate(line) {
        AsmLine::Equ { name, expr }
    } else if let Some(label) = line.strip_suffix(':') {
        AsmLine::Label(label.to_owned())
    } else {
        AsmLine::Instruction(line.to_owned())
    };
    Some(LocatedAsmLine {
        line: line_number,
        column,
        kind,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DataWidth {
    Byte,
    Word,
}

fn parse_equate(line: &str) -> Option<(String, String)> {
    if let Some(rest) = line.strip_prefix(".equ ") {
        let (name, expr) = rest.split_once(',')?;
        return Some((name.trim().to_owned(), expr.trim().to_owned()));
    }
    if let Some((name, expr)) = line.split_once(" equ ") {
        return Some((name.trim().to_owned(), expr.trim().to_owned()));
    }
    if let Some((name, expr)) = line.split_once('=') {
        let name = name.trim();
        if looks_like_label_ref(name) {
            return Some((name.to_owned(), expr.trim().to_owned()));
        }
    }
    None
}

fn parse_data_values(values: &str, width: DataWidth) -> Vec<DataValue> {
    split_data_values(values)
        .into_iter()
        .map(|value| {
            let value = value.trim();
            if let Some(bytes) = parse_quoted_bytes(value) {
                DataValue::Bytes(bytes)
            } else {
                match width {
                    DataWidth::Byte => DataValue::Byte(value.to_owned()),
                    DataWidth::Word => DataValue::Word(value.to_owned()),
                }
            }
        })
        .collect()
}

fn split_data_values(values: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in values.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                current.push(ch);
            }
            ',' if !quoted => {
                out.push(current.trim().to_owned());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_owned());
    }
    out
}

fn parse_quoted_bytes(value: &str) -> Option<Vec<u8>> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.as_bytes().to_vec())
}

fn line_location(instruction: &LocatedAsmLine, options: &AssemblerSourceOptions) -> SourceLocation {
    SourceLocation {
        file: options
            .source_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("<assembly>")),
        line: instruction.line,
        column: instruction.column,
    }
}

fn line_diagnostic(
    instruction: &LocatedAsmLine,
    options: &AssemblerSourceOptions,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic::at(line_location(instruction, options), message)
}

fn data_len(values: &[DataValue]) -> usize {
    values
        .iter()
        .map(|value| match value {
            DataValue::Byte(_) => 1,
            DataValue::Word(_) => 2,
            DataValue::Bytes(bytes) => bytes.len(),
        })
        .sum()
}

fn emit_data(
    values: &[DataValue],
    labels: &HashMap<String, u32>,
    pc: u32,
    bytes: &mut Vec<u8>,
) -> Result<(), Diagnostic> {
    for value in values {
        match value {
            DataValue::Byte(expr) => bytes.push(eval_u8(expr, labels, pc)?),
            DataValue::Word(expr) => push16(bytes, eval_expr(expr, labels, pc)?)?,
            DataValue::Bytes(raw) => bytes.extend(raw),
        }
    }
    Ok(())
}

fn append_org_padding(bytes: &mut Vec<u8>, pc: u32, new_pc: u32) -> Result<(), Diagnostic> {
    if new_pc < pc {
        return Err(Diagnostic::new(format!(
            "org target 0x{new_pc:06X} is before current address 0x{pc:06X}"
        )));
    }
    let padding = usize::try_from(new_pc - pc)
        .map_err(|_| Diagnostic::new("org padding exceeds host addressable memory"))?;
    bytes.resize(bytes.len() + padding, 0);
    Ok(())
}

fn eval_u8(expr: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u8, Diagnostic> {
    let value = eval_expr(expr, labels, pc)?;
    if value > 0xFF {
        return Err(Diagnostic::new(format!("value {expr} is outside u8 range")));
    }
    Ok(value as u8)
}

fn eval_expr(expr: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u32, Diagnostic> {
    let mut parts = expr.split_whitespace();
    let Some(first) = parts.next() else {
        return Err(Diagnostic::new("empty assembly expression"));
    };
    let mut value = eval_atom(first, labels, pc)? as i64;
    while let Some(op) = parts.next() {
        let rhs = parts
            .next()
            .ok_or_else(|| Diagnostic::new(format!("missing operand after `{op}`")))?;
        let rhs = eval_atom(rhs, labels, pc)? as i64;
        match op {
            "+" => value += rhs,
            "-" => value -= rhs,
            _ => {
                return Err(Diagnostic::new(format!(
                    "unsupported assembly operator `{op}`"
                )));
            }
        }
    }
    if !(0..=Address24::MAX as i64).contains(&value) {
        return Err(Diagnostic::new(format!(
            "assembly expression `{expr}` is outside the 24-bit address space"
        )));
    }
    Ok(value as u32)
}

fn eval_atom(text: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u32, Diagnostic> {
    let text = text.trim().trim_end_matches(',');
    if text == "$" {
        return Ok(pc & 0xFF_FFFF);
    }
    if let Some(value) = labels.get(text).copied() {
        return Ok(value);
    }
    parse_number(text)
}

fn instruction_len(cpu: AssemblerCpu, text: &str) -> Result<usize, Diagnostic> {
    if let Some(len) = asm_meta::generated_instruction_len(cpu, text)? {
        Ok(len)
    } else if !cpu.supports_z80_syntax() {
        Err(Diagnostic::new(format!(
            "test assembler does not support instruction `{text}`"
        )))
    } else if matches!(text, "sra a" | "srl a" | "rl a" | "rr a") {
        Ok(2)
    } else if text.starts_with("ld h,") || text.starts_with("ld a,") {
        Ok(2)
    } else if text.starts_with("xor ") {
        Ok(2)
    } else {
        Err(Diagnostic::new(format!(
            "test assembler does not support instruction `{text}`"
        )))
    }
}

fn emit_instruction(
    cpu: AssemblerCpu,
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    bytes: &mut Vec<u8>,
) -> Result<(), Diagnostic> {
    if let Some(generated) = asm_meta::encode_generated_instruction(cpu, text)? {
        bytes.extend(generated);
    } else if let Some(branch) = asm_meta::branch_instruction(cpu, text) {
        bytes.push(branch.opcode);
        let target = parse_addr(branch.target, labels, pc)?;
        match branch.width {
            asm_meta::BranchWidth::Relative8 => bytes.push(relative_offset(pc, target)?),
            asm_meta::BranchWidth::Absolute16 => push16(bytes, target)?,
            asm_meta::BranchWidth::Absolute24 => push24(bytes, target),
        }
    } else if !cpu.supports_z80_syntax() {
        return Err(Diagnostic::new(format!(
            "test assembler does not support instruction `{text}`"
        )));
    } else if let Some(direct) = asm_meta::direct24_instruction(cpu, text) {
        bytes.extend_from_slice(direct.prefix);
        push24(bytes, parse_addr(direct.addr, labels, pc)?);
    } else if let Some(load) = asm_meta::imm24_load_instruction(cpu, text) {
        bytes.extend_from_slice(load.prefix);
        push24(bytes, parse_addr(load.value, labels, pc)?);
    } else if let Some(value) = text.strip_prefix("ld h,") {
        bytes.push(0x26);
        bytes.push(parse_u8(value.trim())?);
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

fn push16(bytes: &mut Vec<u8>, value: u32) -> Result<(), Diagnostic> {
    if value > 0xFFFF {
        return Err(Diagnostic::new(format!(
            "address operand 0x{value:X} is outside the 16-bit address space"
        )));
    }
    bytes.push(value as u8);
    bytes.push((value >> 8) as u8);
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

fn parse_addr(text: &str, labels: &HashMap<String, u32>, pc: u32) -> Result<u32, Diagnostic> {
    match eval_expr(text, labels, pc) {
        Ok(value) if value <= Address24::MAX => Ok(value),
        Ok(_) => Err(Diagnostic::new(format!(
            "address operand `{text}` is outside the 24-bit address space"
        ))),
        Err(_) if looks_like_label_ref(text) => {
            Err(Diagnostic::new(format!("unknown assembly label `{text}`")))
        }
        Err(error) if error.message.contains("outside the 24-bit address space") => {
            Err(Diagnostic::new(format!(
                "address operand `{text}` is outside the 24-bit address space"
            )))
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
    let value = eval_expr(text, &HashMap::new(), 0)?;
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

    use crate::{
        asm::{AssemblyOptions, emit_ez80_assembly, emit_ez80_assembly_with_options},
        compile::load_program,
        parser::parse_program,
        target::{Address24, EZRA_RAM_BASE},
    };

    use super::*;

    fn compile_and_run_source(source: &str, instruction_budget: u64) -> (String, TestRun) {
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, instruction_budget).unwrap();
        (asm, run)
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ezra_vm_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn z80_assembler_uses_16_bit_absolute_branches() {
        let bytes = assemble_subset_at(CpuFamily::Z80, "call 0005h\njp done\ndone:\nret\n", 0x0100)
            .unwrap();

        assert_eq!(bytes, [0xCD, 0x05, 0x00, 0xC3, 0x06, 0x01, 0xC9]);
    }

    #[test]
    fn cpm_bdos_function_2_outputs_characters_and_function_0_exits() {
        let run = run_assembly_test_with_cpu_options_at(
            CpuFamily::Z80,
            include_str!("../examples/cpm-z80/hello-line.asm"),
            &TestRunOptions {
                instruction_budget: 1_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0xFF00,
            },
            0x0100,
        )
        .unwrap();

        assert!(run.halted, "{run:?}");
        assert_eq!(run.debug_output, b"EZRA\r\n");
        assert_eq!(run.failure, None);
    }

    #[test]
    fn cpm_bdos_function_9_outputs_dollar_terminated_strings() {
        let run = run_assembly_test_with_cpu_options_at(
            CpuFamily::Z80,
            r#"
                ld hl, 010Eh
                ex de, hl
                ld c, 9
                call 0005h
                ld c, 0
                call 0005h
            message:
                db "EZRA CP/M$"
            "#,
            &TestRunOptions {
                instruction_budget: 1_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: 0xFF00,
            },
            0x0100,
        )
        .unwrap();

        assert!(run.halted, "{run:?}");
        assert_eq!(run.debug_output, b"EZRA CP/M");
        assert_eq!(run.failure, None);
    }

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
    fn runs_full_program_core_language_constructs_on_ez80_vm() {
        let source = r#"
            alias Byte = u8
            alias Word = u16

            const LIMIT: Byte = 4
            const EXPECTED: Word = 1 + 9 + 3 + 4 + 9

            struct Pair {
                lo: Byte
                hi: Word
            }

            global values: [Byte; LIMIT] = [1, 2, 3, 4]
            global pair: Pair = Pair { lo: 0, hi: 0 }

            fn add_word(left: Word, right: Word) -> Word {
                return left + right
            }

            fn sum_values() -> Word {
                let index: Byte = 0
                let sum: Word = 0
                while index < LIMIT {
                    sum += cast<Word>(values[index])
                    index += 1
                }
                return sum
            }

            fn main() {
                values[1] = values[1] + 7
                pair.lo = values[1]
                pair.hi = add_word(sum_values(), cast<Word>(pair.lo))

                let ptr: ptr<u8> = &values[1]
                test.assert_eq_u8(*ptr, 9, 1)
                mem.poke8(ptr + 1, 6)
                test.assert_eq_u8(values[2], 6, 2)

                if pair.hi == EXPECTED {
                    test.pass()
                }
                test.fail(3)
            }
        "#;
        let (asm, run) = compile_and_run_source(source, 12_000);

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn runs_full_program_ports_and_volatile_mmio_on_ez80_vm() {
        let source = r#"
            port DEBUG: u8 = 0x0C
            volatile mmio STATUS: ptr<u8> = 0x040270
            volatile mmio CONTROL: ptr<u8> = 0x040271

            fn main() {
                *(CONTROL) = *STATUS + 1
                out DEBUG, *CONTROL
                test.assert_eq_u8(*CONTROL, 0x43, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test_with_options(
            &asm,
            &TestRunOptions {
                instruction_budget: 4_000,
                initial_ports: Vec::new(),
                initial_memory: vec![(0x040270, 0x42)],
                stack_top: EZRA_STACK_TOP.get(),
            },
        )
        .unwrap();

        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"C", "{asm}");
    }

    #[test]
    fn runs_full_program_imports_visibility_embeds_and_assets_on_ez80_vm() {
        let root = temp_root("imports_assets");
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let main_path = root.join("game.ezra");
        std::fs::write(
            root.join("lib/assets.ezra"),
            r#"
                const PRIVATE_OFFSET: u8 = 1
                pub const PUBLIC_OFFSET: u8 = PRIVATE_OFFSET
                pub embed sprite: bytes = bytes [0x41, 0x42, 0x43]

                pub fn second() -> u8 {
                    return *(sprite.ptr + PUBLIC_OFFSET)
                }
            "#,
        )
        .unwrap();
        std::fs::write(
            &main_path,
            r#"
                import lib.assets

                fn main() {
                    test.assert_eq_u24(assets.sprite.len, 3, 1)
                    test.assert_eq_u8(assets.second(), 0x42, 2)
                    test.assert_eq_u8(*(lib.assets.sprite.ptr + 2), 0x43, 3)
                    test.pass()
                }
            "#,
        )
        .unwrap();
        let program = load_program(&main_path).unwrap();
        let asm = emit_ez80_assembly(&program).unwrap();
        let run = run_assembly_test(&asm, 8_000).unwrap();

        let _ = std::fs::remove_dir_all(root);
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
    }

    #[test]
    fn runs_full_program_naked_and_interrupt_functions_on_ez80_vm() {
        let source = r#"
            naked fn raw_debug() {
                asm volatile(clobber a, clobber ports) {
                    "ld a, 0x4E"
                    "out0 (0Ch), a"
                    "ret"
                }
            }

            interrupt fn irq_debug() {
                debug.char('I')
            }

            fn main() {
                raw_debug()
                irq_debug()
                test.pass()
            }
        "#;
        let (asm, run) = compile_and_run_source(source, 6_000);

        assert!(asm.contains("    reti"), "{asm}");
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
        assert_eq!(run.debug_output, b"NI", "{asm}");
    }

    #[test]
    fn runs_full_program_with_custom_layout_addresses_on_ez80_vm() {
        let source = r#"
            global value: u8 = 0x3A

            fn main() {
                test.assert_eq_u8(value, 0x3A, 1)
                test.pass()
            }
        "#;
        let program = parse_program(Path::new("game.ezra"), source).unwrap();
        let load_addr = Address24::new(0x040000);
        let entry_addr = Address24::new(0x040040);
        let code_base = Address24::new(0x040040);
        let asm = emit_ez80_assembly_with_options(
            &program,
            AssemblyOptions {
                load_addr,
                entry_addr,
                code_base,
                ram_base: Address24::new(0x050000),
                ..AssemblyOptions::default()
            },
        )
        .unwrap();
        let run = run_assembly_test_with_options_at(
            &asm,
            &TestRunOptions {
                instruction_budget: 4_000,
                initial_ports: Vec::new(),
                initial_memory: Vec::new(),
                stack_top: EZRA_STACK_TOP.get(),
            },
            load_addr.get(),
        )
        .unwrap();

        assert!(asm.contains("ld (050000h), a"), "{asm}");
        assert!(
            !asm.contains(&format!("{:06X}h", EZRA_RAM_BASE.get())),
            "{asm}"
        );
        assert!(run.halted, "{asm}");
        assert_eq!(run.result_code, 0, "{asm}");
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
    fn assembles_register_exchange_instructions() {
        let bytes = assemble_ez80_subset_at("ex de, hl\nexx\n", EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0xEB, 0xD9]);
    }

    #[test]
    fn assembles_interrupt_return_instructions() {
        let bytes = assemble_ez80_subset_at("reti\nretn\n", EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0xED, 0x4D, 0xED, 0x45]);
    }

    #[test]
    fn assembles_restart_instructions() {
        let asm = "rst 00h\nrst 08h\nrst 10h\nrst 18h\nrst 20h\nrst 28h\nrst 30h\nrst 38h\n";
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0xC7, 0xCF, 0xD7, 0xDF, 0xE7, 0xEF, 0xF7, 0xFF]);
    }

    #[test]
    fn assembles_lis_restart_instructions() {
        let bytes = assemble_ez80_subset_at("rst.lis 10h\n", EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0x49, 0xD7]);
    }

    #[test]
    fn assembles_common_control_and_special_register_instructions() {
        let asm = r#"
            halt
            im 0
            im 1
            im 2
            rld
            rrd
            ld i, a
            ld r, a
            ld a, i
            ld a, r
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0x76, 0xED, 0x46, 0xED, 0x56, 0xED, 0x5E, 0xED, 0x6F, 0xED, 0x67, 0xED, 0x47, 0xED,
                0x4F, 0xED, 0x57, 0xED, 0x5F,
            ]
        );
    }

    #[test]
    fn assembles_more_16_bit_register_instructions() {
        let asm = r#"
            inc bc
            inc de
            inc hl
            inc sp
            dec bc
            dec de
            dec hl
            dec sp
            adc hl, bc
            adc hl, de
            adc hl, hl
            adc hl, sp
            sbc hl, hl
            sbc hl, sp
            ld sp, hl
            ld sp, ix
            ld sp, iy
            ex (sp), hl
            ex (sp), ix
            ex (sp), iy
            jp (hl)
            jp (ix)
            jp (iy)
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0x03, 0x13, 0x23, 0x33, 0x0B, 0x1B, 0x2B, 0x3B, 0xED, 0x4A, 0xED, 0x5A, 0xED, 0x6A,
                0xED, 0x7A, 0xED, 0x62, 0xED, 0x72, 0xF9, 0xDD, 0xF9, 0xFD, 0xF9, 0xE3, 0xDD, 0xE3,
                0xFD, 0xE3, 0xE9, 0xDD, 0xE9, 0xFD, 0xE9,
            ]
        );
    }

    #[test]
    fn assembles_hl_indirect_alu_and_cb_instructions() {
        let asm = r#"
            add a, (hl)
            adc a, (hl)
            sub (hl)
            sbc a, (hl)
            and (hl)
            xor (hl)
            or (hl)
            cp (hl)
            inc (hl)
            dec (hl)
            rlc (hl)
            rrc (hl)
            rl (hl)
            rr (hl)
            sla (hl)
            sra (hl)
            srl (hl)
            bit 0, (hl)
            res 1, (hl)
            set 7, (hl)
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0x86, 0x8E, 0x96, 0x9E, 0xA6, 0xAE, 0xB6, 0xBE, 0x34, 0x35, 0xCB, 0x06, 0xCB, 0x0E,
                0xCB, 0x16, 0xCB, 0x1E, 0xCB, 0x26, 0xCB, 0x2E, 0xCB, 0x3E, 0xCB, 0x46, 0xCB, 0x8E,
                0xCB, 0xFE,
            ]
        );
    }

    #[test]
    fn assembles_ix_iy_indexed_load_store_and_alu_forms() {
        let asm = r#"
            ld b, (ix+1)
            ld c, (iy-2)
            ld (ix+3), d
            ld (iy-4), e
            ld (ix+5), 7Fh
            inc (iy+6)
            dec (ix-7)
            add a, (ix+8)
            adc a, (iy+9)
            sub (ix+10)
            sbc a, (iy+11)
            and (ix+12)
            xor (iy+13)
            or (ix+14)
            cp (iy+15)
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xDD, 0x46, 0x01, 0xFD, 0x4E, 0xFE, 0xDD, 0x72, 0x03, 0xFD, 0x73, 0xFC, 0xDD, 0x36,
                0x05, 0x7F, 0xFD, 0x34, 0x06, 0xDD, 0x35, 0xF9, 0xDD, 0x86, 0x08, 0xFD, 0x8E, 0x09,
                0xDD, 0x96, 0x0A, 0xFD, 0x9E, 0x0B, 0xDD, 0xA6, 0x0C, 0xFD, 0xAE, 0x0D, 0xDD, 0xB6,
                0x0E, 0xFD, 0xBE, 0x0F,
            ]
        );
    }

    #[test]
    fn assembles_ix_iy_indexed_cb_forms() {
        let asm = r#"
            rlc (ix+1)
            rr (iy-2)
            bit 3, (ix+4)
            res 2, (iy+5)
            set 7, (ix-6)
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xDD, 0xCB, 0x01, 0x06, 0xFD, 0xCB, 0xFE, 0x1E, 0xDD, 0xCB, 0x04, 0x5E, 0xFD, 0xCB,
                0x05, 0x96, 0xDD, 0xCB, 0xFA, 0xFE,
            ]
        );
    }

    #[test]
    fn assembles_more_ix_iy_16_bit_forms() {
        let asm = r#"
            inc ix
            inc iy
            dec ix
            dec iy
            add ix, bc
            add ix, de
            add ix, ix
            add iy, bc
            add iy, de
            add iy, iy
            ld ix, (040000h)
            ld iy, (040003h)
            ld (040006h), ix
            ld (040009h), iy
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xDD, 0x23, 0xFD, 0x23, 0xDD, 0x2B, 0xFD, 0x2B, 0xDD, 0x09, 0xDD, 0x19, 0xDD, 0x29,
                0xFD, 0x09, 0xFD, 0x19, 0xFD, 0x29, 0xDD, 0x2A, 0x00, 0x00, 0x04, 0xFD, 0x2A, 0x03,
                0x00, 0x04, 0xDD, 0x22, 0x06, 0x00, 0x04, 0xFD, 0x22, 0x09, 0x00, 0x04,
            ]
        );
    }

    #[test]
    fn assembles_ix_iy_byte_alias_forms() {
        let asm = r#"
            ld ixh, 12h
            ld ixl, a
            ld b, ixh
            ld ixh, ixl
            inc ixh
            dec ixl
            add a, ixh
            xor ixl
            ld iyh, 34h
            ld iyl, a
            ld c, iyh
            ld iyh, iyl
            inc iyh
            dec iyl
            adc a, iyh
            cp iyl
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xDD, 0x26, 0x12, 0xDD, 0x6F, 0xDD, 0x44, 0xDD, 0x65, 0xDD, 0x24, 0xDD, 0x2D, 0xDD,
                0x84, 0xDD, 0xAD, 0xFD, 0x26, 0x34, 0xFD, 0x6F, 0xFD, 0x4C, 0xFD, 0x65, 0xFD, 0x24,
                0xFD, 0x2D, 0xFD, 0x8C, 0xFD, 0xBD,
            ]
        );
    }

    #[test]
    fn assembles_full_in0_out0_register_forms() {
        let asm = r#"
            in0 b, (12h)
            in0 c, (12h)
            in0 d, (12h)
            in0 e, (12h)
            in0 h, (12h)
            in0 l, (12h)
            in0 a, (12h)
            out0 (34h), b
            out0 (34h), c
            out0 (34h), d
            out0 (34h), e
            out0 (34h), h
            out0 (34h), l
            out0 (34h), a
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xED, 0x00, 0x12, 0xED, 0x08, 0x12, 0xED, 0x10, 0x12, 0xED, 0x18, 0x12, 0xED, 0x20,
                0x12, 0xED, 0x28, 0x12, 0xED, 0x38, 0x12, 0xED, 0x01, 0x34, 0xED, 0x09, 0x34, 0xED,
                0x11, 0x34, 0xED, 0x19, 0x34, 0xED, 0x21, 0x34, 0xED, 0x29, 0x34, 0xED, 0x39, 0x34,
            ]
        );
    }

    #[test]
    fn assembles_ez80_mode_suffix_prefix_forms() {
        let asm = r#"
            nop.sis
            ld.lis b, a
            xor.sil 55h
            out0.lil (0Ch), a
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0x40, 0x00, 0x49, 0x47, 0x52, 0xEE, 0x55, 0x5B, 0xED, 0x39, 0x0C
            ]
        );
    }

    #[test]
    fn assembles_sp_direct24_loads_and_stores() {
        let asm = r#"
            ld sp, (040000h)
            ld (040003h), sp
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [0xED, 0x7B, 0x00, 0x00, 0x04, 0xED, 0x73, 0x03, 0x00, 0x04]
        );
    }

    #[test]
    fn assembles_standard_io_instructions() {
        let asm = r#"
            in a, (12h)
            out (34h), a
            in b, (c)
            in c, (c)
            in d, (c)
            in e, (c)
            in h, (c)
            in l, (c)
            in a, (c)
            out (c), b
            out (c), c
            out (c), d
            out (c), e
            out (c), h
            out (c), l
            out (c), a
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xDB, 0x12, 0xD3, 0x34, 0xED, 0x40, 0xED, 0x48, 0xED, 0x50, 0xED, 0x58, 0xED, 0x60,
                0xED, 0x68, 0xED, 0x78, 0xED, 0x41, 0xED, 0x49, 0xED, 0x51, 0xED, 0x59, 0xED, 0x61,
                0xED, 0x69, 0xED, 0x79,
            ]
        );
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
                0xED, 0x4B, 0x00, 0x01, 0x04, 0xED, 0x5B, 0x03, 0x01, 0x04, 0xED, 0x43, 0x06, 0x01,
                0x04, 0xED, 0x53, 0x09, 0x01, 0x04,
            ]
        );
    }

    #[test]
    fn assembles_all_direct24_loads_and_stores() {
        let asm = r#"
            ld a, (ix_buffer)
            ld hl, (iy_buffer)
            ld bc, (040006h)
            ld de, (040009h)
            ld ix, (04000Ch)
            ld iy, (04000Fh)
            ld (ix_buffer), a
            ld (iy_buffer), hl
            ld (040018h), bc
            ld (04001Bh), de
            ld (04001Eh), ix
            ld (040021h), iy
        ix_buffer:
            nop
        iy_buffer:
            nop
        "#;
        let bytes = assemble_ez80_subset_at(asm, 0x040000).unwrap();

        assert_eq!(
            bytes,
            [
                0x3A, 0x38, 0x00, 0x04, 0x2A, 0x39, 0x00, 0x04, 0xED, 0x4B, 0x06, 0x00, 0x04, 0xED,
                0x5B, 0x09, 0x00, 0x04, 0xDD, 0x2A, 0x0C, 0x00, 0x04, 0xFD, 0x2A, 0x0F, 0x00, 0x04,
                0x32, 0x38, 0x00, 0x04, 0x22, 0x39, 0x00, 0x04, 0xED, 0x43, 0x18, 0x00, 0x04, 0xED,
                0x53, 0x1B, 0x00, 0x04, 0xDD, 0x22, 0x1E, 0x00, 0x04, 0xFD, 0x22, 0x21, 0x00, 0x04,
                0x00, 0x00,
            ]
        );
    }

    #[test]
    fn direct24_labels_starting_with_index_register_names_are_not_index_indirect() {
        let asm = r#"
            ld a, (ix_label)
            ld hl, (iy_label)
            ld (ix_label), a
            ld (iy_label), hl
        ix_label:
            nop
        iy_label:
            nop
        "#;
        let bytes = assemble_ez80_subset_at(asm, 0x040000).unwrap();

        assert_eq!(
            bytes,
            [
                0x3A, 0x10, 0x00, 0x04, 0x2A, 0x11, 0x00, 0x04, 0x32, 0x10, 0x00, 0x04, 0x22, 0x11,
                0x00, 0x04, 0x00, 0x00,
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
                0xC6, 0x01, 0xCE, 0x02, 0xD6, 0x02, 0xDE, 0x03, 0xE6, 0x03, 0xEE, 0x04, 0xF6, 0x05,
                0xFE, 0x06,
            ]
        );
    }

    #[test]
    fn assembles_misc_accumulator_alu_instructions() {
        let bytes =
            assemble_ez80_subset_at("scf\nccf\ncpl\ndaa\nneg\n", EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0x37, 0x3F, 0x2F, 0x27, 0xED, 0x44]);
    }

    #[test]
    fn assembles_accumulator_rotate_shorthands() {
        let bytes =
            assemble_ez80_subset_at("rlca\nrla\nrrca\nrra\n", EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0x07, 0x17, 0x0F, 0x1F]);
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
    fn runs_misc_accumulator_alu_instructions_on_ez80_vm() {
        let asm = r#"
            scf
            ccf
            jp c, fail
            ld a, 0F0h
            cpl
            cp 0Fh
            jp nz, fail
            neg
            cp 0F1h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 01h
            out0 (0Dh), a
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }

    #[test]
    fn runs_accumulator_rotate_shorthands_on_ez80_vm() {
        let asm = r#"
            ld a, 81h
            rlca
            cp 03h
            jp nz, fail
            rrca
            cp 81h
            jp nz, fail
            rla
            rra
            cp 81h
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 01h
            out0 (0Dh), a
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
    }

    #[test]
    fn assembles_bit_register_instructions() {
        let asm = "bit 0, b\nbit 1, c\nbit 2, d\nbit 3, e\nbit 4, h\nbit 5, l\nbit 7, a\nres 0, b\nset 7, a\n";
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xCB, 0x40, 0xCB, 0x49, 0xCB, 0x52, 0xCB, 0x5B, 0xCB, 0x64, 0xCB, 0x6D, 0xCB, 0x7F,
                0xCB, 0x80, 0xCB, 0xFF,
            ]
        );
    }

    #[test]
    fn runs_bit_register_instructions_on_ez80_vm() {
        let asm = r#"
            ld a, 02h
            set 0, a
            cp 03h
            jp nz, fail
            res 0, a
            cp 02h
            jp nz, fail
            bit 1, a
            jp z, fail
            bit 0, a
            jp nz, fail
            xor a
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a
        fail:
            ld a, 01h
            out0 (0Dh), a
            out0 (0Eh), a
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

        assert!(run.halted);
        assert_eq!(run.result_code, 0);
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
    fn assembles_all_absolute_conditional_jumps() {
        let asm = r#"
            jp nz, target
            jp z, target
            jp nc, target
            jp c, target
            jp po, target
            jp pe, target
            jp p, target
            jp m, target
        target:
            ret
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xC2, 0x20, 0x00, 0x01, 0xCA, 0x20, 0x00, 0x01, 0xD2, 0x20, 0x00, 0x01, 0xDA, 0x20,
                0x00, 0x01, 0xE2, 0x20, 0x00, 0x01, 0xEA, 0x20, 0x00, 0x01, 0xF2, 0x20, 0x00, 0x01,
                0xFA, 0x20, 0x00, 0x01, 0xC9,
            ]
        );
    }

    #[test]
    fn assembles_all_conditional_call_instructions() {
        let asm = r#"
            call nz, target
            call z, target
            call nc, target
            call c, target
            call po, target
            call pe, target
            call p, target
            call m, target
        target:
            ret
        "#;
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(
            bytes,
            [
                0xC4, 0x20, 0x00, 0x01, 0xCC, 0x20, 0x00, 0x01, 0xD4, 0x20, 0x00, 0x01, 0xDC, 0x20,
                0x00, 0x01, 0xE4, 0x20, 0x00, 0x01, 0xEC, 0x20, 0x00, 0x01, 0xF4, 0x20, 0x00, 0x01,
                0xFC, 0x20, 0x00, 0x01, 0xC9,
            ]
        );
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
    fn assembles_equates_data_strings_and_org_padding() {
        let bytes = assemble_ez80_subset_at(
            r#"
            VALUE equ 41h
            db VALUE, "BC"
            dw VALUE + 1
            org 010008h
            db 44h
            "#,
            0x010000,
        )
        .unwrap();

        assert_eq!(
            bytes,
            [0x41, 0x42, 0x43, 0x42, 0x00, 0x00, 0x00, 0x00, 0x44]
        );
    }

    #[test]
    fn assembles_address_expressions_in_instructions() {
        let bytes = assemble_ez80_subset_at(
            r#"
            TARGET = 010010h
            jp TARGET + 2
            "#,
            0x010000,
        )
        .unwrap();

        assert_eq!(bytes, [0xC3, 0x12, 0x00, 0x01]);
    }

    #[test]
    fn z80n_and_z180_inherit_z80_instruction_encoding() {
        for cpu in [AssemblerCpu::Z80N, AssemblerCpu::Z180] {
            let assembled =
                assemble_subset_with_symbols_at(cpu, "ld a, 7Fh\nret\n", 0x0100).unwrap();

            assert_eq!(assembled.bytes, [0x3E, 0x7F, 0xC9]);
        }
    }

    #[test]
    fn z180_and_ez80_accept_z180_lineage_instructions() {
        for cpu in [AssemblerCpu::Z180, AssemblerCpu::Ez80] {
            let assembled =
                assemble_subset_with_symbols_at(cpu, "mlt bc\nout0 (34h), a\n", 0x0100).unwrap();

            assert_eq!(assembled.bytes, [0xED, 0x4C, 0xED, 0x39, 0x34]);
        }
    }

    #[test]
    fn z80_and_z80n_reject_z180_ez80_only_instructions() {
        for cpu in [AssemblerCpu::Z80, AssemblerCpu::Z80N] {
            let error = assemble_subset_with_symbols_at(cpu, "mlt bc\n", 0x0100).unwrap_err();

            assert_eq!(
                error.message,
                "test assembler does not support instruction `mlt bc`"
            );
        }
    }

    #[test]
    fn i8080_and_i8085_accept_intel_8080_mnemonics() {
        for cpu in [AssemblerCpu::I8080, AssemblerCpu::I8085] {
            let assembled = assemble_subset_with_symbols_at(
                cpu,
                r#"
                lxi h, 1234h
                mvi a, 42h
                mov m, a
                inr m
                dad h
                xchg
                xthl
                sphl
                pchl
                start:
                jnz start
                call start
                ret
                "#,
                0x0100,
            )
            .unwrap();

            assert_eq!(
                assembled.bytes,
                [
                    0x21, 0x34, 0x12, 0x3E, 0x42, 0x77, 0x34, 0x29, 0xEB, 0xE3, 0xF9, 0xE9, 0xC2,
                    0x0C, 0x01, 0xCD, 0x0C, 0x01, 0xC9,
                ]
            );
        }
    }

    #[test]
    fn i8085_accepts_rim_sim_but_i8080_rejects_them() {
        let assembled =
            assemble_subset_with_symbols_at(AssemblerCpu::I8085, "rim\nsim\n", 0x0100).unwrap();
        assert_eq!(assembled.bytes, [0x20, 0x30]);

        let error =
            assemble_subset_with_symbols_at(AssemblerCpu::I8080, "rim\n", 0x0100).unwrap_err();
        assert_eq!(
            error.message,
            "test assembler does not support instruction `rim`"
        );
    }

    #[test]
    fn i8080_rejects_z80_extension_syntax() {
        let error = assemble_subset_with_symbols_at(AssemblerCpu::I8080, "ld a, 7Fh\n", 0x0100)
            .unwrap_err();

        assert_eq!(
            error.message,
            "test assembler does not support instruction `ld a, 7Fh`"
        );
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
    fn rejects_invalid_restart_targets() {
        for (asm, expected) in [
            (
                "rst 07h\n",
                "restart target 0x7 is not one of 0x00, 0x08, ..., 0x38",
            ),
            (
                "rst 40h\n",
                "restart target 0x40 is not one of 0x00, 0x08, ..., 0x38",
            ),
        ] {
            let error = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap_err();

            assert_eq!(error.message, expected);
        }
    }

    #[test]
    fn rejects_invalid_bit_register_operands() {
        for (asm, expected) in [
            ("bit 8, a\n", "bit index 8 is outside 0..7"),
            ("bit 0, ix\n", "invalid bit register `ix`"),
            ("set 8, a\n", "bit index 8 is outside 0..7"),
            ("res 0, ix\n", "invalid bit register `ix`"),
        ] {
            let error = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap_err();

            assert_eq!(error.message, expected);
        }
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
    fn assembles_all_conditional_return_instructions() {
        let asm = "ret nz\nret z\nret nc\nret c\nret po\nret pe\nret p\nret m\n";
        let bytes = assemble_ez80_subset_at(asm, EZRA_LOAD_ADDR.get()).unwrap();

        assert_eq!(bytes, [0xC0, 0xC8, 0xD0, 0xD8, 0xE0, 0xE8, 0xF0, 0xF8]);
    }

    #[test]
    fn runs_non_maskable_interrupt_return_on_ez80_vm() {
        let asm = r#"
            call raw_return
            ld a, 00h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        raw_return:
            retn
        "#;
        let run = run_assembly_test(asm, 100).unwrap();

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
    fn runs_sign_conditional_absolute_jumps_on_ez80_vm() {
        let asm = r#"
            ld a, 80h
            or a
            jp m, negative
            ld a, 10h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        negative:
            ld a, 00h
            or a
            jp p, positive
            ld a, 11h
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        positive:
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
    fn runs_conditional_calls_on_ez80_vm() {
        let asm = r#"
            xor a
            call z, mark_taken
            call nz, fail
            out0 (0Dh), a
            ld a, 01h
            out0 (0Eh), a

        mark_taken:
            ld a, 00h
            ret

        fail:
            ld a, 20h
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
            "assembly instruction at 0x1000000 with length 0x1 exceeds the 24-bit address space"
        );
    }

    #[test]
    fn rejects_assembly_that_exceeds_address_space() {
        let error = assemble_ez80_subset_at("nop\nnop\n", 0xFF_FFFF).unwrap_err();

        assert_eq!(
            error.message,
            "assembly instruction at 0x1000000 with length 0x1 exceeds the 24-bit address space"
        );
    }

    #[test]
    fn rejects_assembly_labels_outside_address_space() {
        let error = assemble_ez80_subset_at("nop\nend:\n", 0xFF_FFFF).unwrap_err();

        assert_eq!(
            error.message,
            "assembly label `end` address 0x1000000 is outside the 24-bit address space"
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
