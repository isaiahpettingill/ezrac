use std::{
    cell::Cell,
    collections::{BTreeMap, HashMap, HashSet},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
};

use ez80::{Cpu, CpuMode, Machine, Reg8, Reg16};

#[cfg(feature = "m68k")]
use crate::asm::m68k as asm_m68k;
use crate::asm::{avr, chip8 as chip8_asm, ez80 as asm_meta, m6800};
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
    pub section_bases: Vec<AssemblySymbol>,
    pub line_origins: Vec<SourceLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestRunOptions {
    pub instruction_budget: u64,
    pub initial_ports: Vec<(u8, u8)>,
    pub initial_memory: Vec<(u32, u8)>,
    pub stack_top: u32,
}

/// A fully assembled test image that an emulator backend can load and execute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestImage {
    pub cpu_family: CpuFamily,
    pub base_addr: u32,
    pub bytes: Vec<u8>,
}

/// Extensible execution backend for target test images.
///
/// The built-in backend uses the `ez80` crate for eZ80, Z80, Z80N, Z180,
/// i8080, and i8085. Other CPU families can supply their own backend without
/// changing the compiler test command.
pub trait EmulatorBackend: Send + Sync {
    fn supports(&self, cpu_family: CpuFamily) -> bool;
    fn run(&self, image: &TestImage, options: &TestRunOptions) -> Result<TestRun, Diagnostic>;
}

pub struct TestRunner {
    backends: Vec<Box<dyn EmulatorBackend>>,
}

impl Default for TestRunner {
    fn default() -> Self {
        Self::new(vec![Box::new(Ez80Emulator)])
    }
}

impl TestRunner {
    pub fn new(backends: Vec<Box<dyn EmulatorBackend>>) -> Self {
        Self { backends }
    }

    pub fn run(&self, image: &TestImage, options: &TestRunOptions) -> Result<TestRun, Diagnostic> {
        let backend = self
            .backends
            .iter()
            .find(|backend| backend.supports(image.cpu_family))
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "no test emulator is registered for CPU `{}`",
                    image.cpu_family.as_str()
                ))
            })?;
        backend.run(image, options)
    }
}

pub struct Ez80Emulator;

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
    let code = assemble_subset_at(cpu_family, assembly, base_addr)?;
    TestRunner::default().run(
        &TestImage {
            cpu_family,
            base_addr,
            bytes: code,
        },
        options,
    )
}

impl EmulatorBackend for Ez80Emulator {
    fn supports(&self, cpu_family: CpuFamily) -> bool {
        matches!(
            cpu_family,
            CpuFamily::Ez80
                | CpuFamily::Z80
                | CpuFamily::Z80N
                | CpuFamily::Z180
                | CpuFamily::I8080
                | CpuFamily::I8085
        )
    }

    fn run(&self, image: &TestImage, options: &TestRunOptions) -> Result<TestRun, Diagnostic> {
        let address_limit = address_limit_for_family(image.cpu_family);
        if image.base_addr > address_limit {
            return Err(Diagnostic::new(format!(
                "test image base address 0x{:X} is outside the {}-bit address space",
                image.base_addr,
                address_width_for_family(image.cpu_family)
            )));
        }
        if options.stack_top > address_limit {
            return Err(Diagnostic::new(format!(
                "test stack top 0x{:X} is outside the {}-bit address space",
                options.stack_top,
                address_width_for_family(image.cpu_family)
            )));
        }
        for (address, _) in &options.initial_memory {
            if *address > address_limit {
                return Err(Diagnostic::new(format!(
                    "test memory address 0x{address:X} is outside the {}-bit address space",
                    address_width_for_family(image.cpu_family)
                )));
            }
        }

        let code_start = image.base_addr;
        let code_end =
            checked_code_end_for_family(code_start, image.bytes.len(), image.cpu_family)?;
        let mut machine = TestMachine::new(address_limit);
        for (port, value) in &options.initial_ports {
            machine.ports[*port as usize] = *value;
        }
        for (address, value) in &options.initial_memory {
            machine.poke(*address, *value);
        }
        for (address, byte) in image.bytes.iter().copied().enumerate() {
            machine.poke(image.base_addr + address as u32, byte);
        }

        let mut cpu = Cpu::new_for_mode(cpu_mode_for_family(image.cpu_family));
        cpu.state.reg.adl = image.cpu_family == CpuFamily::Ez80;
        cpu.state.set_pc(image.base_addr);
        if image.cpu_family != CpuFamily::Ez80 {
            cpu.state.reg.set16(Reg16::SP, options.stack_top as u16);
        } else {
            cpu.state.reg.set24(Reg16::SP, options.stack_top);
        }
        if std::env::var_os("EZRA_TRACE_VM").is_some() {
            cpu.set_trace(true);
        }

        for instruction in 0..options.instruction_budget {
            let pc = cpu.state.pc();
            if is_cpm_cpu(image.cpu_family) && handle_cpm_bdos_call(&mut cpu, &mut machine)? {
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
            let sp = if image.cpu_family == CpuFamily::Ez80 {
                cpu.state.reg.get24(Reg16::SP)
            } else {
                cpu.state.reg.get16(Reg16::SP) as u32
            };
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
}

fn address_width_for_family(cpu_family: CpuFamily) -> u8 {
    if cpu_family == CpuFamily::Ez80 {
        24
    } else {
        16
    }
}

fn address_limit_for_family(cpu_family: CpuFamily) -> u32 {
    match cpu_family {
        CpuFamily::Ez80 => Address24::MAX,
        CpuFamily::Chip8 | CpuFamily::SuperChip => 0x0FFF,
        _ => u16::MAX as u32,
    }
}

fn checked_code_end_for_family(
    code_start: u32,
    code_len: usize,
    cpu_family: CpuFamily,
) -> Result<u32, Diagnostic> {
    let end = checked_code_end(code_start, code_len)?;
    if end > address_limit_for_family(cpu_family) {
        return Err(Diagnostic::new(format!(
            "test program exceeds the {}-bit address space",
            address_width_for_family(cpu_family)
        )));
    }
    Ok(end)
}

fn is_cpm_cpu(cpu_family: CpuFamily) -> bool {
    matches!(
        cpu_family,
        CpuFamily::Z80 | CpuFamily::I8080 | CpuFamily::I8085
    )
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
        CpuFamily::Lr35902
        | CpuFamily::M6800
        | CpuFamily::Mos6502
        | CpuFamily::Chip8
        | CpuFamily::SuperChip
        | CpuFamily::XoChip => CpuMode::Z80,
        CpuFamily::Avr => CpuMode::Z80,
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
    if !cpu.is_enabled() {
        return Err(Diagnostic::new(format!(
            "assembler CPU `{}` requires the `{}` Cargo feature",
            cpu.as_str(),
            cpu.feature_name()
        )));
    }
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
    let mut declared_names = HashSet::new();
    for symbol in &options.symbols {
        labels.insert(symbol.name.clone(), symbol.addr);
        declared_names.insert(symbol.name.clone());
    }
    let mut pending_equates = Vec::new();
    let begins_with_section = instructions
        .first()
        .is_some_and(|line| matches!(line.kind, AsmLine::Section(_)));
    let default_pc = if begins_with_section {
        base_addr
    } else {
        section_base(options, ".text").unwrap_or(base_addr)
    } & 0xFF_FFFF;
    let mut pc = default_pc;

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
                if !declared_names.insert(name.clone()) {
                    return Err(line_diagnostic(
                        instruction,
                        options,
                        format!("duplicate assembly label `{name}`"),
                    ));
                }
                labels.insert(name.clone(), pc);
            }
            AsmLine::Equ { name, expr } => {
                if !declared_names.insert(name.clone()) {
                    return Err(line_diagnostic(
                        instruction,
                        options,
                        format!("duplicate assembly symbol `{name}`"),
                    ));
                }
                pending_equates.push((instruction.clone(), name.clone(), expr.clone(), pc));
            }
            AsmLine::Section(name) => {
                if let Some(base) = section_base(options, name) {
                    pc = base;
                }
            }
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

    while !pending_equates.is_empty() {
        let known = labels.clone().into_iter().collect::<HashMap<_, _>>();
        let mut unresolved = Vec::new();
        let mut progress = false;
        for (instruction, name, expr, equ_pc) in pending_equates {
            match eval_expr(&expr, &known, equ_pc) {
                Ok(value) => {
                    labels.insert(name, value);
                    progress = true;
                }
                Err(_) => unresolved.push((instruction, name, expr, equ_pc)),
            }
        }
        if !progress {
            let (instruction, _, expr, equ_pc) = &unresolved[0];
            return Err(eval_expr(expr, &known, *equ_pc)
                .unwrap_err()
                .with_location_if_missing(line_location(instruction, options)));
        }
        pending_equates = unresolved;
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
    if default_pc != pc {
        append_org_padding(&mut bytes, pc, default_pc)?;
        pc = default_pc;
    }
    for instruction in instructions {
        match &instruction.kind {
            AsmLine::Label(_) | AsmLine::Equ { .. } => {}
            AsmLine::Section(name) => {
                if let Some(base) = section_base(options, name) {
                    append_org_padding(&mut bytes, pc, base).map_err(|error| {
                        error.with_location_if_missing(line_location(&instruction, options))
                    })?;
                    pc = base;
                }
            }
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

fn section_base(options: &AssemblerSourceOptions, name: &str) -> Option<u32> {
    options
        .section_bases
        .iter()
        .find(|section| section.name == name)
        .map(|section| section.addr)
}

pub fn measure_assembly(cpu: AssemblerCpu, assembly: &str) -> Result<usize, Diagnostic> {
    measure_assembly_with_options(cpu, assembly, &AssemblerSourceOptions::default())
}

pub fn measure_assembly_with_options(
    cpu: AssemblerCpu,
    assembly: &str,
    options: &AssemblerSourceOptions,
) -> Result<usize, Diagnostic> {
    let mut len = 0usize;
    for instruction in assembly
        .lines()
        .enumerate()
        .filter_map(|(index, line)| parse_line(index + 1, line))
    {
        let item_len = match instruction.kind {
            AsmLine::Data(ref values) => data_len(values),
            AsmLine::Instruction(ref text) => instruction_len(cpu, text).map_err(|error| {
                error.with_location_if_missing(line_location(&instruction, options))
            })?,
            AsmLine::Label(_) | AsmLine::Equ { .. } | AsmLine::Section(_) | AsmLine::Org(_) => 0,
        };
        len = len
            .checked_add(item_len)
            .ok_or_else(|| Diagnostic::new("assembly size exceeds host addressable memory"))?;
    }
    Ok(len)
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
    if let Some(origin) = options.line_origins.get(instruction.line.saturating_sub(1)) {
        return SourceLocation {
            file: origin.file.clone(),
            line: origin.line,
            column: origin
                .column
                .saturating_add(instruction.column.saturating_sub(1)),
        };
    }
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
    if let Some(value) = labels
        .iter()
        .find_map(|(name, value)| name.eq_ignore_ascii_case(text).then_some(*value))
    {
        return Ok(value);
    }
    parse_number(text)
}

fn instruction_len(cpu: AssemblerCpu, text: &str) -> Result<usize, Diagnostic> {
    if cpu == AssemblerCpu::Lr35902 {
        return Ok(encode_lr35902(text, &HashMap::new(), 0, false)?.len());
    }
    if cpu == AssemblerCpu::Avr {
        return avr::instruction_len(text);
    }
    if let Some(dialect) = chip8_dialect(cpu) {
        return chip8_asm::instruction_len(dialect, text);
    }
    if cpu == AssemblerCpu::M6800 {
        return m6800::instruction_len(text)?.ok_or_else(|| {
            Diagnostic::new(format!(
                "assembler does not support M6800 instruction `{text}`"
            ))
        });
    }
    #[cfg(feature = "m68k")]
    if cpu == AssemblerCpu::M68k {
        return asm_m68k::instruction_len(text);
    }
    if cpu == AssemblerCpu::Mos6502 {
        return crate::asm::mos6502::instruction_len(text);
    }
    if let Some((opcode, _)) = z80_imm16_load(cpu, text) {
        let prefix_len = usize::from(opcode == 0xDD || opcode == 0xFD);
        return Ok(prefix_len + 3);
    }
    asm_meta::generated_instruction_len(cpu, text)?.ok_or_else(|| {
        Diagnostic::new(format!(
            "test assembler does not support instruction `{text}`"
        ))
    })
}

fn emit_instruction(
    cpu: AssemblerCpu,
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    bytes: &mut Vec<u8>,
) -> Result<(), Diagnostic> {
    if cpu == AssemblerCpu::Lr35902 {
        bytes.extend(encode_lr35902(text, labels, pc, true)?);
        return Ok(());
    }
    if cpu == AssemblerCpu::Avr {
        bytes.extend(avr::encode_instruction(text, labels, pc)?);
        return Ok(());
    }
    if let Some(dialect) = chip8_dialect(cpu) {
        bytes.extend(chip8_asm::encode_instruction(dialect, text, labels, pc)?);
        return Ok(());
    }
    if cpu == AssemblerCpu::M6800 {
        let Some(encoded) = m6800::emit_instruction(text, labels, pc)? else {
            return Err(Diagnostic::new(format!(
                "assembler does not support M6800 instruction `{text}`"
            )));
        };
        bytes.extend(encoded);
        return Ok(());
    }
    #[cfg(feature = "m68k")]
    if cpu == AssemblerCpu::M68k {
        bytes.extend(asm_m68k::encode(text, labels, pc, true)?);
        return Ok(());
    }
    if cpu == AssemblerCpu::Mos6502 {
        bytes.extend(crate::asm::mos6502::encode_instruction(
            text, labels, pc, true,
        )?);
        return Ok(());
    }
    if let Some((opcode, value)) = z80_imm16_load(cpu, text) {
        if opcode == 0xDD || opcode == 0xFD {
            bytes.push(opcode);
            bytes.push(0x21);
        } else {
            bytes.push(opcode);
        }
        push16(bytes, parse_addr(value, labels, pc)?)?;
    } else if let Some((prefix, base)) = asm_meta::ez80_mode_suffixed_instruction(cpu, text) {
        bytes.push(prefix);
        emit_instruction(cpu, &base, labels, pc + 1, bytes)?;
    } else if let Some(generated) = asm_meta::encode_generated_instruction(cpu, text)? {
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
    } else {
        return Err(Diagnostic::new(format!(
            "test assembler does not support instruction `{text}`"
        )));
    }
    Ok(())
}

fn z80_imm16_load(cpu: AssemblerCpu, text: &str) -> Option<(u8, &str)> {
    if !matches!(
        cpu,
        AssemblerCpu::Z80 | AssemblerCpu::Z80N | AssemblerCpu::Z180
    ) {
        return None;
    }
    let (destination, value) = text.strip_prefix("ld ")?.split_once(',')?;
    let opcode = match destination.trim() {
        "bc" => 0x01,
        "de" => 0x11,
        "hl" => 0x21,
        "sp" => 0x31,
        "ix" => 0xDD,
        "iy" => 0xFD,
        _ => return None,
    };
    let value = value.trim();
    if value.starts_with('(')
        || matches!(
            value,
            "a" | "b" | "c" | "d" | "e" | "h" | "l" | "bc" | "de" | "hl" | "sp" | "ix" | "iy"
        )
    {
        return None;
    }
    Some((opcode, value))
}

fn chip8_dialect(cpu: AssemblerCpu) -> Option<chip8_asm::Chip8Dialect> {
    match cpu {
        AssemblerCpu::Chip8 => Some(chip8_asm::Chip8Dialect::Chip8),
        AssemblerCpu::SuperChip => Some(chip8_asm::Chip8Dialect::SuperChip),
        AssemblerCpu::XoChip => Some(chip8_asm::Chip8Dialect::XoChip),
        _ => None,
    }
}

fn encode_lr35902(
    text: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let text = text.trim().to_ascii_lowercase();
    let text = text.as_str();
    let fixed: Option<&[u8]> = match text {
        "nop" => Some(&[0x00][..]),
        "rlca" => Some(&[0x07]),
        "rrca" => Some(&[0x0F]),
        "stop" => Some(&[0x10, 0x00]),
        "rla" => Some(&[0x17]),
        "rra" => Some(&[0x1F]),
        "daa" => Some(&[0x27]),
        "cpl" => Some(&[0x2F]),
        "scf" => Some(&[0x37]),
        "ccf" => Some(&[0x3F]),
        "halt" => Some(&[0x76]),
        "ret" => Some(&[0xC9]),
        "reti" => Some(&[0xD9]),
        "jp hl" | "jp (hl)" => Some(&[0xE9]),
        "di" => Some(&[0xF3]),
        "ld sp, hl" => Some(&[0xF9]),
        "ei" => Some(&[0xFB]),
        _ => None,
    };
    if let Some(bytes) = fixed {
        return Ok(bytes.to_vec());
    }

    if let Some((operation, operand)) = split_mnemonic(text) {
        if let Some(code) = lr_cb_operation(operation, operand)? {
            return Ok(vec![0xCB, code]);
        }
        if let Some(code) = lr_inc_dec(operation, operand) {
            return Ok(vec![code]);
        }
        if operation == "ld" {
            return encode_lr_load(operand, labels, pc, resolve);
        }
        if operation == "ldi" {
            return encode_lr_load(&operand.replace("(hl)", "(hl+)"), labels, pc, resolve);
        }
        if operation == "ldd" {
            return encode_lr_load(&operand.replace("(hl)", "(hl-)"), labels, pc, resolve);
        }
        if operation == "ldh" {
            let Some((dst, src)) = operand.split_once(',') else {
                return Err(Diagnostic::new(format!("invalid ldh syntax `{operand}`")));
            };
            let (dst, src) = (dst.trim(), src.trim());
            if dst == "(c)" && src == "a" {
                return Ok(vec![0xE2]);
            }
            if dst == "a" && src == "(c)" {
                return Ok(vec![0xF2]);
            }
            let (opcode, value) = if dst.starts_with('(') && src == "a" {
                (0xE0, &dst[1..dst.len() - 1])
            } else if dst == "a" && src.starts_with('(') {
                (0xF0, &src[1..src.len() - 1])
            } else {
                return Err(Diagnostic::new(format!("invalid ldh syntax `{operand}`")));
            };
            let address = lr_value(value, labels, pc, resolve)?;
            let offset = if address >= 0xFF00 {
                address - 0xFF00
            } else {
                address
            };
            let offset = u8::try_from(offset).map_err(|_| {
                Diagnostic::new(format!("LDH address `{value}` is outside FF00h..FFFFh"))
            })?;
            return Ok(vec![opcode, offset]);
        }
        if let Some(bytes) = encode_lr_alu(operation, operand, labels, pc, resolve)? {
            return Ok(bytes);
        }
        if let Some(bytes) = encode_lr_control(operation, operand, labels, pc, resolve)? {
            return Ok(bytes);
        }
        if matches!(operation, "push" | "pop") {
            let register = lr_stack_register(operand).ok_or_else(|| {
                Diagnostic::new(format!("invalid LR35902 stack register `{operand}`"))
            })?;
            return Ok(vec![
                if operation == "push" { 0xC5 } else { 0xC1 } + register * 0x10,
            ]);
        }
    }
    Err(Diagnostic::new(format!(
        "assembler does not support LR35902 instruction `{text}`"
    )))
}

fn split_mnemonic(text: &str) -> Option<(&str, &str)> {
    text.split_once(char::is_whitespace)
        .map(|(op, rest)| (op, rest.trim()))
}

fn lr_r8(value: &str) -> Option<u8> {
    match value.trim() {
        "b" => Some(0),
        "c" => Some(1),
        "d" => Some(2),
        "e" => Some(3),
        "h" => Some(4),
        "l" => Some(5),
        "(hl)" => Some(6),
        "a" => Some(7),
        _ => None,
    }
}

fn lr_r16(value: &str) -> Option<u8> {
    match value.trim() {
        "bc" => Some(0),
        "de" => Some(1),
        "hl" => Some(2),
        "sp" => Some(3),
        _ => None,
    }
}

fn lr_stack_register(value: &str) -> Option<u8> {
    match value.trim() {
        "bc" => Some(0),
        "de" => Some(1),
        "hl" => Some(2),
        "af" => Some(3),
        _ => None,
    }
}

fn lr_condition(value: &str) -> Option<u8> {
    match value.trim() {
        "nz" => Some(0),
        "z" => Some(1),
        "nc" => Some(2),
        "c" => Some(3),
        _ => None,
    }
}

fn lr_cb_operation(operation: &str, operand: &str) -> Result<Option<u8>, Diagnostic> {
    if matches!(operation, "bit" | "res" | "set") {
        let Some((bit, register)) = operand.split_once(',') else {
            return Ok(None);
        };
        let bit = bit
            .trim()
            .parse::<u8>()
            .map_err(|_| Diagnostic::new(format!("invalid bit index `{bit}`")))?;
        if bit > 7 {
            return Err(Diagnostic::new(format!("bit index {bit} is outside 0..7")));
        }
        let register = lr_r8(register)
            .ok_or_else(|| Diagnostic::new(format!("invalid LR35902 register `{register}`")))?;
        let base = match operation {
            "bit" => 0x40,
            "res" => 0x80,
            _ => 0xC0,
        };
        return Ok(Some(base + bit * 8 + register));
    }
    let row = match operation {
        "rlc" => 0,
        "rrc" => 1,
        "rl" => 2,
        "rr" => 3,
        "sla" => 4,
        "sra" => 5,
        "swap" => 6,
        "srl" => 7,
        _ => return Ok(None),
    };
    let register = lr_r8(operand)
        .ok_or_else(|| Diagnostic::new(format!("invalid LR35902 register `{operand}`")))?;
    Ok(Some(row * 8 + register))
}

fn lr_inc_dec(operation: &str, operand: &str) -> Option<u8> {
    let increment = operation == "inc";
    if !increment && operation != "dec" {
        return None;
    }
    if let Some(register) = lr_r8(operand) {
        return Some((if increment { 0x04 } else { 0x05 }) + register * 8);
    }
    lr_r16(operand).map(|register| (if increment { 0x03 } else { 0x0B }) + register * 0x10)
}

fn lr_value(
    expr: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<u32, Diagnostic> {
    if resolve {
        eval_expr(expr.trim(), labels, pc)
    } else {
        Ok(0)
    }
}

fn lr_u8(
    expr: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<u8, Diagnostic> {
    let value = lr_value(expr, labels, pc, resolve)?;
    u8::try_from(value).map_err(|_| Diagnostic::new(format!("value {expr} is outside u8 range")))
}

fn lr_u16(
    expr: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<[u8; 2], Diagnostic> {
    let value = lr_value(expr, labels, pc, resolve)?;
    let value = u16::try_from(value)
        .map_err(|_| Diagnostic::new(format!("value {expr} is outside u16 range")))?;
    Ok(value.to_le_bytes())
}

fn encode_lr_load(
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Vec<u8>, Diagnostic> {
    let Some((dst, src)) = operand.split_once(',') else {
        return Err(Diagnostic::new(format!("invalid ld syntax `{operand}`")));
    };
    let (dst, src) = (dst.trim(), src.trim());
    if let (Some(dst), Some(src)) = (lr_r8(dst), lr_r8(src)) {
        if dst == 6 && src == 6 {
            return Err(Diagnostic::new("use `halt`, not `ld (hl), (hl)`"));
        }
        return Ok(vec![0x40 + dst * 8 + src]);
    }
    if let Some(dst) = lr_r8(dst)
        && !src.starts_with('(')
    {
        let value = lr_u8(src, labels, pc, resolve)?;
        return Ok(vec![0x06 + dst * 8, value]);
    }
    if dst == "hl" && (src.starts_with("sp+") || src.starts_with("sp-")) {
        return Ok(vec![0xF8, parse_lr_signed(&src[2..])? as u8]);
    }
    if let Some(register) = lr_r16(dst) {
        if dst == "sp" && src == "hl" {
            return Ok(vec![0xF9]);
        }
        let value = lr_u16(src, labels, pc, resolve)?;
        return Ok(vec![0x01 + register * 0x10, value[0], value[1]]);
    }
    let indirect = match (dst, src) {
        ("(bc)", "a") => Some(0x02),
        ("(de)", "a") => Some(0x12),
        ("(hl+)", "a") | ("(hli)", "a") => Some(0x22),
        ("(hl-)", "a") | ("(hld)", "a") => Some(0x32),
        ("a", "(bc)") => Some(0x0A),
        ("a", "(de)") => Some(0x1A),
        ("a", "(hl+)") | ("a", "(hli)") => Some(0x2A),
        ("a", "(hl-)") | ("a", "(hld)") => Some(0x3A),
        _ => None,
    };
    if let Some(opcode) = indirect {
        return Ok(vec![opcode]);
    }
    if dst == "(c)" && src == "a" {
        return Ok(vec![0xE2]);
    }
    if dst == "a" && src == "(c)" {
        return Ok(vec![0xF2]);
    }
    if dst.starts_with('(') && dst.ends_with(')') {
        let address = &dst[1..dst.len() - 1];
        let value = lr_u16(address, labels, pc, resolve)?;
        return match src {
            "a" => Ok(vec![0xEA, value[0], value[1]]),
            "sp" => Ok(vec![0x08, value[0], value[1]]),
            _ => Err(Diagnostic::new(format!("invalid LR35902 load `{operand}`"))),
        };
    }
    if dst == "a" && src.starts_with('(') && src.ends_with(')') {
        let value = lr_u16(&src[1..src.len() - 1], labels, pc, resolve)?;
        return Ok(vec![0xFA, value[0], value[1]]);
    }
    Err(Diagnostic::new(format!("invalid LR35902 load `{operand}`")))
}

fn encode_lr_alu(
    operation: &str,
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    if operation == "add" && operand.starts_with("hl,") {
        let register = lr_r16(operand[3..].trim())
            .ok_or_else(|| Diagnostic::new(format!("invalid add operand `{operand}`")))?;
        return Ok(Some(vec![0x09 + register * 0x10]));
    }
    if operation == "add" && operand.starts_with("sp,") {
        return Ok(Some(vec![
            0xE8,
            parse_lr_signed(operand[3..].trim())? as u8,
        ]));
    }
    let (row, source) = match operation {
        "add" => (0, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "adc" => (1, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "sub" => (2, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "sbc" => (3, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "and" => (4, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "xor" => (5, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "or" => (6, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        "cp" => (7, operand.strip_prefix("a,").unwrap_or(operand).trim()),
        _ => return Ok(None),
    };
    if let Some(register) = lr_r8(source) {
        return Ok(Some(vec![0x80 + row * 8 + register]));
    }
    Ok(Some(vec![
        0xC6 + row * 8,
        lr_u8(source, labels, pc, resolve)?,
    ]))
}

fn encode_lr_control(
    operation: &str,
    operand: &str,
    labels: &HashMap<String, u32>,
    pc: u32,
    resolve: bool,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    if operation == "rst" {
        let vector = lr_u8(operand, labels, pc, resolve)?;
        if vector > 0x38 || vector % 8 != 0 {
            return Err(Diagnostic::new(
                "LR35902 restart vector must be 00h..38h in steps of 8",
            ));
        }
        return Ok(Some(vec![0xC7 + vector]));
    }
    if operation == "ret" {
        return Ok(lr_condition(operand).map(|condition| vec![0xC0 + condition * 8]));
    }
    if !matches!(operation, "jr" | "jp" | "call") {
        return Ok(None);
    }
    let (condition, target) = operand
        .split_once(',')
        .map_or((None, operand), |(condition, target)| {
            (lr_condition(condition), target.trim())
        });
    if operand.contains(',') && condition.is_none() {
        return Err(Diagnostic::new(format!(
            "invalid branch condition `{operand}`"
        )));
    }
    if operation == "jr" {
        let opcode = condition.map_or(0x18, |condition| 0x20 + condition * 8);
        let target = lr_value(target, labels, pc, resolve)?;
        let offset = if resolve {
            relative_offset(pc, target)?
        } else {
            0
        };
        return Ok(Some(vec![opcode, offset]));
    }
    let value = lr_u16(target, labels, pc, resolve)?;
    let opcode = match (operation, condition) {
        ("jp", None) => 0xC3,
        ("call", None) => 0xCD,
        ("jp", Some(c)) => 0xC2 + c * 8,
        ("call", Some(c)) => 0xC4 + c * 8,
        _ => unreachable!(),
    };
    Ok(Some(vec![opcode, value[0], value[1]]))
}

fn parse_lr_signed(text: &str) -> Result<i8, Diagnostic> {
    let text = text.trim();
    let value = if let Some(rest) = text.strip_prefix('-') {
        -(parse_number(rest)? as i32)
    } else if let Some(rest) = text.strip_prefix('+') {
        parse_number(rest)? as i32
    } else {
        parse_number(text)? as i32
    };
    i8::try_from(value)
        .map_err(|_| Diagnostic::new(format!("signed byte `{text}` is outside -128..127")))
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

pub(crate) fn relative_offset(pc: u32, target: u32) -> Result<u8, Diagnostic> {
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

pub(crate) fn parse_number(text: &str) -> Result<u32, Diagnostic> {
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
    address_mask: u32,
    ports: [u8; 256],
    cycles: Cell<i64>,
    halted: bool,
    result_code: u8,
    debug_output: Vec<u8>,
}

impl TestMachine {
    fn new(address_limit: u32) -> Self {
        Self {
            memory: HashMap::new(),
            address_mask: address_limit,
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
            .get(&(address & self.address_mask))
            .copied()
            .unwrap_or(0)
    }

    fn poke(&mut self, address: u32, value: u8) {
        self.memory.insert(address & self.address_mask, value);
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
mod tests;
