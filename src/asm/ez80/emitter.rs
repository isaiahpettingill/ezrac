use crate::compat::{SourcePath, prelude::*};

#[cfg(feature = "std")]
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    asm::ez80::{analyze_instruction, instruction_effects, is_unsupported_z80_family_instruction},
    ast::{
        AccessPath, AccessSegment, AssignOp, BinaryOp, Declaration, Expr, Function, Place, Program,
        Stmt, Type, UnaryOp,
    },
    declaration::unwrapped_declaration,
    diagnostic::Diagnostic,
    hir::HirProgram,
    intrinsics::{
        BitsIntrinsic, CATALOG, IntIntrinsic, IntrinsicError, IntrinsicOperation,
        IntrinsicResolution, MemIntrinsic, ResultCount, VolatilePolicy,
    },
    regalloc::{
        Location, PhysReg, PhysicalRegister, RegClass, RegUnit, RegisterClass, RegisterUnit,
        SpillClass, SpillClassId, Target,
        source::{SourceLocal, allocate_source_locals},
    },
    target::{
        Address24, AssemblerCpu, CpuFamily, EZRA_ASSET_BASE, EZRA_AUDIO_BASE, EZRA_CODE_BASE,
        EZRA_ENTRY_ADDR, EZRA_LOAD_ADDR, EZRA_RAM_BASE, EZRA_RODATA_BASE, EZRA_STACK_TOP,
        EZRA_VRAM_BASE,
    },
    tbir::{
        TbirProgram,
        cost::{CostCandidate, CostModel, FlagEffects, FlagSet, InstructionCost},
    },
};

mod intel8080;
mod symbols;

use crate::asm::{
    comments::{access_path_summary, stmt_summary, type_display, with_readability_comments},
    data::terminated_text_data_line,
    reachability::{RoutineProfile, strip_unreachable_generated_routines_with_roots},
};
use intel8080::{is_intel_8080_family, translate_assembly_for_cpu};
use symbols::{FunctionSig, StaticLiveness, StructLayout, Symbols, ValueWidth, Variable};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum RuntimeHelper {
    Pass,
    Fail,
    Memcpy,
    Memset,
    MulU8,
    MulU16,
    MulU24,
    MulI24,
    DivU8,
    DivU16,
    DivU24,
    DivI24,
    ModU8,
    ModU16,
    ModU24,
    ModI24,
}

const RUNTIME_HELPER_ORDER: [RuntimeHelper; 16] = [
    RuntimeHelper::Pass,
    RuntimeHelper::Fail,
    RuntimeHelper::Memcpy,
    RuntimeHelper::Memset,
    RuntimeHelper::MulU8,
    RuntimeHelper::MulU16,
    RuntimeHelper::MulU24,
    RuntimeHelper::MulI24,
    RuntimeHelper::DivU8,
    RuntimeHelper::DivU16,
    RuntimeHelper::DivU24,
    RuntimeHelper::DivI24,
    RuntimeHelper::ModU8,
    RuntimeHelper::ModU16,
    RuntimeHelper::ModU24,
    RuntimeHelper::ModI24,
];

impl RuntimeHelper {
    fn label(self) -> &'static str {
        match self {
            Self::Pass => "__ezra_pass",
            Self::Fail => "__ezra_fail",
            Self::Memcpy => "__ezra_memcpy",
            Self::Memset => "__ezra_memset",
            Self::MulU8 => "__ezra_mul_u8",
            Self::MulU16 => "__ezra_mul_u16",
            Self::MulU24 => "__ezra_mul_u24",
            Self::MulI24 => "__ezra_mul_i24",
            Self::DivU8 => "__ezra_div_u8",
            Self::DivU16 => "__ezra_div_u16",
            Self::DivU24 => "__ezra_div_u24",
            Self::DivI24 => "__ezra_div_i24",
            Self::ModU8 => "__ezra_mod_u8",
            Self::ModU16 => "__ezra_mod_u16",
            Self::ModU24 => "__ezra_mod_u24",
            Self::ModI24 => "__ezra_mod_i24",
        }
    }

    fn dependencies(self) -> &'static [Self] {
        match self {
            Self::MulI24 => &[Self::MulU24],
            _ => &[],
        }
    }
}

pub fn emit_ez80_assembly(program: &Program) -> Result<String, Diagnostic> {
    emit_ez80_assembly_with_options(program, AssemblyOptions::default())
}

pub fn emit_ez80_assembly_with_debug_comments(
    program: &Program,
    debug_comments: bool,
) -> Result<String, Diagnostic> {
    emit_ez80_assembly_with_options(
        program,
        AssemblyOptions {
            debug_comments,
            ..AssemblyOptions::default()
        },
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GameBoyBankingMapper {
    Mbc1,
    Mbc5,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GameBoyBankingOptions {
    pub mapper: GameBoyBankingMapper,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssemblyOptions {
    pub cpu: CpuFamily,
    pub debug_comments: bool,
    pub default_sdk_symbols: bool,
    pub dos_executable: bool,
    pub mos_executable: bool,
    pub c64_executable: bool,
    pub ti_os_executable: bool,
    /// Emit the ATmega32U4 hardware vector table used by the Arduboy target.
    pub arduboy_executable: bool,
    /// Game Boy-only explicit ROM banking configuration.
    pub gameboy_banking: Option<GameBoyBankingOptions>,
    /// Optimization level and per-pass overrides used while lowering TBIR.
    pub optimization: crate::optimization::OptimizationOptions,
    pub load_addr: Address24,
    pub entry_addr: Address24,
    pub code_base: Address24,
    pub stack_top: Address24,
    pub ram_base: Address24,
    pub vram_base: Address24,
    pub audio_base: Address24,
    pub asset_base: Address24,
    pub rodata_base: Address24,
    pub section_bases: Vec<(String, Address24)>,
}

impl Default for AssemblyOptions {
    fn default() -> Self {
        Self {
            cpu: CpuFamily::Ez80,
            debug_comments: false,
            default_sdk_symbols: true,
            dos_executable: false,
            mos_executable: false,
            c64_executable: false,
            ti_os_executable: false,
            arduboy_executable: false,
            gameboy_banking: None,
            optimization: crate::optimization::OptimizationOptions::default(),
            load_addr: EZRA_LOAD_ADDR,
            entry_addr: EZRA_ENTRY_ADDR,
            code_base: EZRA_CODE_BASE,
            stack_top: EZRA_STACK_TOP,
            ram_base: EZRA_RAM_BASE,
            vram_base: EZRA_VRAM_BASE,
            audio_base: EZRA_AUDIO_BASE,
            asset_base: EZRA_ASSET_BASE,
            rodata_base: EZRA_RODATA_BASE,
            section_bases: vec![
                (".rodata".to_owned(), EZRA_RODATA_BASE),
                (".assets".to_owned(), EZRA_ASSET_BASE),
            ],
        }
    }
}

pub fn emit_ez80_assembly_with_options(
    program: &Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    let result = (|| {
        let checked = CheckedEz80Program::from_program(program, &options)?;
        emit_ez80_assembly_from_checked(program, &checked, options)
    })();
    result.map_err(|error| locate_program_diagnostic(program, error))
}

fn validate_source_program_before_optimization(
    program: &Program,
    options: &AssemblyOptions,
) -> Result<(), Diagnostic> {
    let symbols = Symbols::from_program(program, options.clone(), None)?;
    let main = program
        .main_function()
        .ok_or_else(|| Diagnostic::new("missing required `fn main()`"))?;
    validate_main_signature(main)?;
    validate_all_function_calls(program, &symbols.functions)?;
    let recursive_call_edges = recursive_call_edges(program, &symbols.functions);
    validate_all_function_bodies(
        program,
        symbols,
        options.clone(),
        recursive_call_edges,
        HashSet::new(),
    )
}

pub fn collect_ez80_semantic_diagnostics(
    program: &Program,
    options: AssemblyOptions,
) -> Vec<Diagnostic> {
    let symbols = match Symbols::from_program(program, options.clone(), None) {
        Ok(symbols) => symbols,
        Err(error) => return vec![error],
    };
    let mut diagnostics = Vec::new();
    for declaration in &program.declarations {
        let Declaration::Function(function) = unwrapped_declaration(declaration) else {
            continue;
        };
        collect_stmt_call_diagnostics(
            &function.body,
            &function.body_spans,
            &symbols.functions,
            &mut diagnostics,
        );

        let mut emitter = Emitter::new(
            symbols.clone(),
            options.clone(),
            recursive_call_edges(program, &symbols.functions),
            HashSet::new(),
            None,
        );
        emitter.disable_dead_code_elimination();
        if let Err(error) = emitter.emit_function(function) {
            let error = locate_program_diagnostic(program, error);
            let error = if error.span.is_none() {
                function
                    .body_spans
                    .first()
                    .map(|span| error.clone().with_span_if_missing(span.span.clone()))
                    .unwrap_or(error)
            } else {
                error
            };
            if !diagnostics.iter().any(|diagnostic| {
                diagnostic.message == error.message && diagnostic.span == error.span
            }) {
                diagnostics.push(error);
            }
        }
    }
    diagnostics
}

fn locate_program_diagnostic(program: &Program, error: Diagnostic) -> Diagnostic {
    if error.location().is_some() {
        return error;
    }
    let quoted = error
        .message
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    let value = error
        .message
        .strip_prefix("value ")
        .and_then(|message| message.split_whitespace().next());
    program
        .declarations
        .iter()
        .filter_map(|declaration| match unwrapped_declaration(declaration) {
            Declaration::Function(function) => Some(function),
            _ => None,
        })
        .flat_map(|function| statement_references(&function.body_spans))
        .filter(|reference| {
            quoted.iter().any(|token| reference.text == *token)
                || value.is_some_and(|value| reference.text == value)
        })
        .min_by_key(|reference| {
            (
                reference
                    .span
                    .end
                    .line
                    .saturating_sub(reference.span.start.line),
                reference
                    .span
                    .end
                    .column
                    .saturating_sub(reference.span.start.column),
            )
        })
        .map(|reference| error.clone().with_span_if_missing(reference.span.clone()))
        .unwrap_or(error)
}

fn statement_references(spans: &[crate::ast::StmtSpan]) -> Vec<&crate::ast::SourceReference> {
    let mut references = Vec::new();
    for span in spans {
        references.extend(span.references.iter());
        references.extend(statement_references(&span.children));
    }
    references
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckedEz80Program {
    pub hir: HirProgram,
    pub tbir: TbirProgram,
}

impl CheckedEz80Program {
    pub fn from_program(program: &Program, options: &AssemblyOptions) -> Result<Self, Diagnostic> {
        validate_source_program_before_optimization(program, options)?;
        let hir = HirProgram::from_ast(program)?;
        let tbir = TbirProgram::lower(&hir, program, options)?;
        Ok(Self { hir, tbir })
    }
}

pub fn emit_ez80_assembly_from_checked(
    original_program: &Program,
    checked: &CheckedEz80Program,
    options: AssemblyOptions,
) -> Result<String, Diagnostic> {
    let lowered_program = &checked.tbir.lowered_program;
    let preserve_function_pointer_locals =
        program_contains_function_pointer_locals(original_program, &options)?;
    let program = if preserve_function_pointer_locals {
        original_program
    } else {
        lowered_program
    };
    debug_assert_eq!(checked.hir.source_path, program.source_path);
    let tail_call_edges = checked.tbir.optimizations.tail_call_edges();
    let analysis_symbols = Symbols::from_program(program, options.clone(), None)?;
    let main = program
        .main_function()
        .ok_or_else(|| Diagnostic::new("missing required `fn main()`"))?;
    validate_main_signature(main)?;
    validate_all_function_calls(program, &analysis_symbols.functions)?;
    let recursive_call_edges = recursive_call_edges(program, &analysis_symbols.functions);
    let dead_code = options
        .optimization
        .is_enabled(crate::optimization::OptimizationPass::DeadCodeElimination);
    let mut emitted_functions = if dead_code {
        reachable_function_names(program, &analysis_symbols)
    } else {
        program
            .declarations
            .iter()
            .filter_map(|declaration| match unwrapped_declaration(declaration) {
                Declaration::Function(function) => Some(function.name.clone()),
                _ => None,
            })
            .collect()
    };
    let mut function_references = Vec::new();
    for declaration in &program.declarations {
        match unwrapped_declaration(declaration) {
            Declaration::Global(global) => {
                collect_expr_function_references(&global.value, &mut function_references)
            }
            Declaration::Function(function) => {
                collect_stmt_function_references(&function.body, &mut function_references)
            }
            _ => {}
        }
    }
    emitted_functions.extend(
        function_references
            .iter()
            .filter(|name| analysis_symbols.functions.contains_key(*name))
            .cloned(),
    );
    let static_liveness = static_liveness(program, &emitted_functions);
    let mut validation_emitter = Emitter::new(
        analysis_symbols.clone(),
        options.clone(),
        recursive_call_edges.clone(),
        tail_call_edges.clone(),
        None,
    );
    validation_emitter.emit_global_initializers(program)?;
    let symbols = Symbols::from_program(program, options.clone(), Some(&static_liveness))?;
    let opaque_assembly = emitted_functions.iter().any(|name| {
        function_declaration(program, name)
            .is_some_and(|function| function_contains_inline_asm(&function.body))
    }) || program.declarations.iter().any(|declaration| {
        matches!(
            unwrapped_declaration(declaration),
            Declaration::ExternAsmFunction(_)
        )
    });
    let cpu = options.cpu;

    let mut emitter = Emitter::new(
        symbols,
        options.clone(),
        recursive_call_edges,
        tail_call_edges,
        Some(static_liveness),
    );
    emitter.emit_prelude();
    emitter.emit_embed_initializers();
    emitter.emit_string_literal_initializers();
    emitter.emit_global_initializers(program)?;
    emitter.emit_start_tail();
    emitter.emit_function(main)?;
    for declaration in &program.declarations {
        let Declaration::Function(function) = unwrapped_declaration(declaration) else {
            continue;
        };
        if function.name != "main" && emitted_functions.contains(&function.name) {
            emitter.emit_function(function)?;
        }
    }
    emitter.emit_function_pointer_trampolines(program, &emitted_functions)?;
    emitter.emit_required_sections();
    let assembly = peephole_cleanup_with_ranges(
        &emitter.out,
        &emitter.cacheable_ranges,
        options
            .optimization
            .is_enabled(crate::optimization::OptimizationPass::RedundantRegisterCopies),
    );
    translate_assembly_for_cpu(cpu, &assembly).map(|asm| {
        let profile = if is_intel_8080_family(cpu) {
            RoutineProfile::Z80
        } else {
            RoutineProfile::Ez80
        };
        let mut assembly_roots = program
            .declarations
            .iter()
            .filter_map(|declaration| match unwrapped_declaration(declaration) {
                Declaration::Function(function)
                    if emitted_functions.contains(&function.name)
                        && (opaque_assembly
                            || has_attr(function, "extern")
                            || has_attr(function, "naked")
                            || has_attr(function, "interrupt")
                            || declaration_is_banked(declaration)) =>
                {
                    Some(function_label(&function.name))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for name in &function_references {
            let Some(signature) = analysis_symbols.functions.get(name) else {
                continue;
            };
            assembly_roots.push(if signature.uses_arg_slots {
                function_pointer_label(name)
            } else {
                function_label(name)
            });
        }
        assembly_roots.sort();
        assembly_roots.dedup();
        let assembly_root_refs = assembly_roots
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let asm = if dead_code {
            strip_unreachable_generated_routines_with_roots(&asm, profile, &assembly_root_refs)
        } else {
            asm
        };
        let asm = if dead_code {
            cleanup_lowered_cfg(&asm, &assembly_root_refs)
        } else {
            asm
        };
        with_readability_comments(
            asm,
            original_program,
            &options,
            "ez80",
            &checked.tbir.source_comments,
        )
    })
}

fn is_z80_family_16bit(cpu: CpuFamily) -> bool {
    matches!(
        cpu,
        CpuFamily::Z80
            | CpuFamily::R800
            | CpuFamily::Z80N
            | CpuFamily::Z180
            | CpuFamily::I8080
            | CpuFamily::I8085
    )
}

fn supports_z80_bit_instructions(cpu: CpuFamily) -> bool {
    matches!(
        cpu,
        CpuFamily::Z80 | CpuFamily::R800 | CpuFamily::Z80N | CpuFamily::Z180 | CpuFamily::Ez80
    )
}

const EZ80_MEMORY_LOCAL_CLASS: RegClass = RegClass(0);
const EZ80_STATIC_SPILL_CLASS: SpillClassId = SpillClassId(0);

fn ez80_local_target(cpu: CpuFamily) -> Target {
    let mut units = ["a", "f", "b", "c", "d", "e", "h", "l", "sp"]
        .into_iter()
        .map(RegisterUnit::new)
        .collect::<Vec<_>>();
    let mut registers = vec![
        PhysicalRegister::new("a", vec![RegUnit(0)]),
        PhysicalRegister::new("f", vec![RegUnit(1)]),
        PhysicalRegister::new("af", vec![RegUnit(0), RegUnit(1)]),
        PhysicalRegister::new("b", vec![RegUnit(2)]),
        PhysicalRegister::new("c", vec![RegUnit(3)]),
        PhysicalRegister::new("bc", vec![RegUnit(2), RegUnit(3)]),
        PhysicalRegister::new("d", vec![RegUnit(4)]),
        PhysicalRegister::new("e", vec![RegUnit(5)]),
        PhysicalRegister::new("de", vec![RegUnit(4), RegUnit(5)]),
        PhysicalRegister::new("h", vec![RegUnit(6)]),
        PhysicalRegister::new("l", vec![RegUnit(7)]),
        PhysicalRegister::new("hl", vec![RegUnit(6), RegUnit(7)]),
        PhysicalRegister::new("sp", vec![RegUnit(8)]),
    ];
    if !is_intel_8080_family(cpu) {
        let ix = units.len();
        units.push(RegisterUnit::new("ix"));
        registers.push(PhysicalRegister::new("ix", vec![RegUnit(ix)]));
        let iy = units.len();
        units.push(RegisterUnit::new("iy"));
        registers.push(PhysicalRegister::new("iy", vec![RegUnit(iy)]));
    }

    Target {
        units,
        registers,
        register_classes: vec![RegisterClass::new("memory-local", Vec::new())],
        spill_classes: vec![
            SpillClass::new("static", None, 1)
                .with_base_alignment(1)
                .for_register_classes(vec![EZ80_MEMORY_LOCAL_CLASS]),
        ],
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ConstantMulForm {
    ShiftAdd { digits: Vec<i8>, negate: bool },
    NativeU8,
    Helper,
}

fn constant_mul_factor(value: i64, width: ValueWidth) -> Option<(u32, u32, bool)> {
    let bits = u32::from(width.bytes()) * 8;
    let modulus = 1_u64 << bits;
    let mask = modulus - 1;
    let raw = ((value as i128) & i128::from(mask)) as u32;
    if raw <= 1 {
        return None;
    }
    let half = 1_u32 << (bits - 1);
    if raw > half {
        Some((raw, (modulus as u32).wrapping_sub(raw), true))
    } else {
        Some((raw, raw, false))
    }
}

fn binary_constant_mul_digits(value: u32) -> Vec<i8> {
    let highest_bit = 31 - value.leading_zeros();
    (0..highest_bit)
        .rev()
        .map(|bit| if value & (1 << bit) != 0 { 1 } else { 0 })
        .collect()
}

fn naf_constant_mul_digits(value: u32) -> Vec<i8> {
    let mut value = value;
    let mut digits = Vec::new();
    while value != 0 {
        if value & 1 == 0 {
            digits.push(0);
            value >>= 1;
        } else {
            let digit = if value & 3 == 1 { 1 } else { -1 };
            digits.push(digit);
            value = if digit == 1 {
                (value - 1) / 2
            } else {
                value.div_ceil(2)
            };
        }
    }
    digits.reverse();
    digits
}

fn constant_mul_shift_add_cost(width: ValueWidth, digits: &[i8], negate: bool) -> InstructionCost {
    let (mut bytes, mut cycles) = if width == ValueWidth::U8 {
        (1_u16, 4_u32)
    } else {
        (2_u16, 12_u32)
    };
    for digit in digits {
        bytes = bytes.saturating_add(1);
        cycles = cycles.saturating_add(4);
        if *digit != 0 {
            if width == ValueWidth::U8 || *digit == 1 {
                bytes = bytes.saturating_add(1);
                cycles = cycles.saturating_add(4);
            } else {
                bytes = bytes.saturating_add(3);
                cycles = cycles.saturating_add(15);
            }
        }
    }
    if negate {
        if width == ValueWidth::U8 {
            bytes = bytes.saturating_add(3);
            cycles = cycles.saturating_add(12);
        } else {
            bytes = bytes.saturating_add(9);
            cycles = cycles.saturating_add(34);
        }
    }
    InstructionCost::new(bytes, cycles, 0, FlagEffects::writes(FlagSet::ALL))
}

fn constant_mul_helper_cost(cpu: CpuFamily, width: ValueWidth, factor: u32) -> InstructionCost {
    let (setup_bytes, setup_cycles) = if width == ValueWidth::U8 {
        (2_u16, 7_u32)
    } else {
        (4_u16, 7_u32)
    };
    let (helper_bytes, helper_cycles) = match (cpu, width) {
        (CpuFamily::Ez80, ValueWidth::U8) => (5_u16, 21_u32),
        (CpuFamily::Ez80, ValueWidth::U16) => (25_u16, 90_u32),
        (CpuFamily::Ez80, ValueWidth::U24) => {
            (24_u16, 25_u32.saturating_add(factor.saturating_mul(55)))
        }
        (_, ValueWidth::U8) => (18_u16, 30_u32.saturating_add(factor.saturating_mul(24))),
        (_, ValueWidth::U16) => (45_u16, 40_u32.saturating_add(factor.saturating_mul(45))),
        (_, ValueWidth::U24) => (24_u16, 25_u32.saturating_add(factor.saturating_mul(55))),
    };
    InstructionCost::new(
        setup_bytes.saturating_add(4).saturating_add(helper_bytes),
        setup_cycles
            .saturating_add(17)
            .saturating_add(helper_cycles),
        0,
        FlagEffects::writes(FlagSet::ALL),
    )
}

fn constant_mul_cost_model(cpu: CpuFamily) -> CostModel {
    let model = if cpu.capabilities().prefer_code_size {
        CostModel::code_size()
    } else {
        CostModel::balanced()
    };
    model.with_live_flags(FlagSet::NONE)
}

#[cfg_attr(not(test), allow(dead_code))]
fn peephole_cleanup(assembly: &str) -> String {
    peephole_cleanup_with_ranges(assembly, &[], true)
}

fn peephole_cleanup_with_ranges(
    assembly: &str,
    cacheable_ranges: &[(u32, u32)],
    eliminate_redundant_register_copies: bool,
) -> String {
    let mut out = String::new();
    let mut register_values = HashMap::<&'static str, String>::new();
    let mut register_copies = HashMap::<&'static str, &'static str>::new();
    let mut cached_memory_loads = HashMap::<(u32, u32), &'static str>::new();
    let mut last_memory_transfer: Option<AbsoluteMemoryTransfer> = None;
    let mut in_inline_asm = false;

    for line in assembly.lines() {
        let trimmed = line.trim();
        if trimmed == "; end asm" {
            in_inline_asm = false;
            register_values.clear();
            register_copies.clear();
            cached_memory_loads.clear();
            last_memory_transfer = None;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if trimmed.starts_with("; asm") {
            in_inline_asm = true;
            register_values.clear();
            register_copies.clear();
            cached_memory_loads.clear();
            last_memory_transfer = None;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let register_copy = (!in_inline_asm && eliminate_redundant_register_copies)
            .then(|| register_copy(trimmed))
            .flatten();
        if register_copy.is_some_and(|(target, source)| {
            target == source
                || register_copies
                    .get(target)
                    .is_some_and(|cached| *cached == source)
        }) {
            continue;
        }
        if is_peephole_block_boundary(trimmed) {
            register_values.clear();
            register_copies.clear();
            cached_memory_loads.clear();
            last_memory_transfer = None;
        }

        let immediate_load = immediate_register_load(line);
        if !in_inline_asm
            && let Some((register, value)) = immediate_load
            && register_values
                .get(register)
                .is_some_and(|cached| cached == value)
        {
            continue;
        }

        let memory_transfer = if in_inline_asm {
            None
        } else {
            parse_absolute_memory_transfer(trimmed)
        };
        let remove_line = if let Some(transfer) = memory_transfer {
            if cacheable_ranges_contain(cacheable_ranges, transfer.address, transfer.width) {
                let redundant_load = !transfer.is_store
                    && cached_memory_loads
                        .get(&(transfer.address, transfer.width))
                        .is_some_and(|register| *register == transfer.register);
                let redundant_transfer = last_memory_transfer.is_some_and(|previous| {
                    previous.address == transfer.address
                        && previous.width == transfer.width
                        && previous.register == transfer.register
                        && previous.is_store != transfer.is_store
                });
                if !redundant_load && !redundant_transfer {
                    last_memory_transfer = Some(transfer);
                }
                redundant_load || redundant_transfer
            } else {
                cached_memory_loads.clear();
                last_memory_transfer = None;
                false
            }
        } else {
            if !trimmed.is_empty() && !trimmed.starts_with(';') {
                last_memory_transfer = None;
            }
            false
        };

        if remove_line {
            continue;
        }

        out.push_str(line);
        out.push('\n');

        if let Some((register, value)) = immediate_load {
            invalidate_register_value_aliases(&mut register_values, register);
            invalidate_register_copy_aliases(&mut register_copies, register);
            register_values.insert(register, value.to_owned());
        } else if !trimmed.is_empty() && !trimmed.starts_with(';') {
            let effects = instruction_effects(trimmed);
            for register in effects.modified_registers {
                invalidate_register_value_aliases(&mut register_values, register);
                invalidate_register_copy_aliases(&mut register_copies, register);
                cached_memory_loads.retain(|_, cached| !registers_overlap(cached, register));
            }
            if let Some((target, source)) = register_copy {
                register_copies.insert(target, source);
            }
            if effects.uses_memory || effects.uses_ports || trimmed.starts_with("call ") {
                cached_memory_loads.clear();
            }
        }
        if let Some(transfer) = memory_transfer
            && cacheable_ranges_contain(cacheable_ranges, transfer.address, transfer.width)
        {
            if transfer.is_store {
                cached_memory_loads.retain(|(address, width), _| {
                    !memory_ranges_overlap(*address, *width, transfer.address, transfer.width)
                });
            } else {
                cached_memory_loads.insert((transfer.address, transfer.width), transfer.register);
            }
        }
        if is_indirect_memory_access(trimmed) {
            // A memory access may be volatile. Do not reuse an immediate
            // address load across it, even when the instruction only changes A.
            register_values.clear();
            register_copies.clear();
            cached_memory_loads.clear();
        }

        if is_peephole_block_terminator(trimmed) {
            register_values.clear();
            register_copies.clear();
            last_memory_transfer = None;
        }
    }

    out
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AbsoluteMemoryTransfer {
    address: u32,
    width: u32,
    register: &'static str,
    is_store: bool,
}

fn immediate_register_load(line: &str) -> Option<(&'static str, &str)> {
    let trimmed = line.trim();
    let (target, value) = trimmed.strip_prefix("ld ")?.split_once(',')?;
    let target = register_name(target.trim())?;
    let value = value.trim();
    (!value.is_empty() && !value.contains('(') && register_name(value).is_none())
        .then_some((target, value))
}

fn register_copy(line: &str) -> Option<(&'static str, &'static str)> {
    let (target, source) = line.strip_prefix("ld ")?.split_once(',')?;
    Some((register_name(target.trim())?, register_name(source.trim())?))
}

fn register_name(register: &str) -> Option<&'static str> {
    match register {
        "a" => Some("a"),
        "b" => Some("b"),
        "c" => Some("c"),
        "d" => Some("d"),
        "e" => Some("e"),
        "h" => Some("h"),
        "l" => Some("l"),
        "hl" => Some("hl"),
        "de" => Some("de"),
        "bc" => Some("bc"),
        "ix" => Some("ix"),
        "iy" => Some("iy"),
        "sp" => Some("sp"),
        _ => None,
    }
}

fn invalidate_register_value_aliases(values: &mut HashMap<&'static str, String>, modified: &str) {
    values.retain(|register, _| !registers_overlap(register, modified));
}

fn invalidate_register_copy_aliases(
    copies: &mut HashMap<&'static str, &'static str>,
    modified: &str,
) {
    copies.retain(|target, source| {
        !registers_overlap(target, modified) && !registers_overlap(source, modified)
    });
}

fn registers_overlap(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    matches!(
        (left, right),
        ("af", "a")
            | ("a", "af")
            | ("hl", "h" | "l")
            | ("de", "d" | "e")
            | ("bc", "b" | "c")
            | ("h" | "l", "hl")
            | ("d" | "e", "de")
            | ("b" | "c", "bc")
    )
}

fn is_indirect_memory_access(line: &str) -> bool {
    line.contains("(hl)")
        || line.contains("(ix")
        || line.contains("(iy")
        || line.contains("(bc)")
        || line.contains("(de)")
}

fn parse_absolute_memory_transfer(line: &str) -> Option<AbsoluteMemoryTransfer> {
    let (target, source) = line.strip_prefix("ld ")?.split_once(',')?;
    let target = target.trim();
    let source = source.trim();
    if let Some(address) = absolute_memory_operand(target) {
        let register = register_name(source)?;
        let width = match register {
            "a" => 1,
            "hl" => 3,
            _ => return None,
        };
        return Some(AbsoluteMemoryTransfer {
            address,
            width,
            register,
            is_store: true,
        });
    }
    if let Some(address) = absolute_memory_operand(source) {
        let register = register_name(target)?;
        let width = match register {
            "a" => 1,
            "hl" => 3,
            _ => return None,
        };
        return Some(AbsoluteMemoryTransfer {
            address,
            width,
            register,
            is_store: false,
        });
    }
    None
}

fn absolute_memory_operand(operand: &str) -> Option<u32> {
    let value = operand.strip_prefix('(')?.strip_suffix(')')?;
    let value = value.strip_suffix('h')?;
    u32::from_str_radix(value, 16).ok()
}

fn cacheable_ranges_contain(ranges: &[(u32, u32)], address: u32, width: u32) -> bool {
    ranges.iter().any(|(start, size)| {
        address >= *start
            && address
                .checked_add(width)
                .is_some_and(|end| end <= start.saturating_add(*size))
    })
}

fn memory_ranges_overlap(
    left_address: u32,
    left_width: u32,
    right_address: u32,
    right_width: u32,
) -> bool {
    let left_end = left_address.saturating_add(left_width);
    let right_end = right_address.saturating_add(right_width);
    left_address < right_end && right_address < left_end
}

fn is_peephole_block_boundary(line: &str) -> bool {
    line.starts_with("section ") || (line.ends_with(':') && !line.starts_with(' '))
}

fn is_peephole_block_terminator(line: &str) -> bool {
    let mnemonic = line.split_whitespace().next().unwrap_or_default();
    matches!(
        mnemonic,
        "call" | "djnz" | "halt" | "jp" | "jr" | "ret" | "reti" | "retn" | "rst"
    )
}

fn cleanup_lowered_cfg(assembly: &str, roots: &[&str]) -> String {
    // Inline assembly may branch to labels or enter code through an address
    // that the compiler cannot inspect. Keep those programs byte-for-byte
    // intact rather than treating an incomplete CFG as proof of dead code.
    if assembly
        .lines()
        .any(|line| line.trim_start().starts_with("; asm"))
    {
        return assembly.to_owned();
    }

    let mut lines = assembly.lines().collect::<Vec<_>>();
    let Some(text_start) = lines.iter().position(|line| line.trim() == "section .text") else {
        return assembly.to_owned();
    };
    let text_end = lines[text_start + 1..]
        .iter()
        .position(|line| line.trim_start().starts_with("section "))
        .map_or(lines.len(), |offset| text_start + 1 + offset);

    let mut label_indices = Vec::new();
    for (index, line) in lines.iter().enumerate().take(text_end).skip(text_start) {
        let trimmed = line.trim();
        let Some(label) = trimmed.strip_suffix(':') else {
            continue;
        };
        if label.is_empty() || label.contains(char::is_whitespace) {
            continue;
        }
        label_indices.push(index);
    }
    if label_indices.is_empty() {
        return assembly.to_owned();
    }

    let mut block_for_label = HashMap::<String, usize>::new();
    for (block, index) in label_indices.iter().enumerate() {
        if let Some(label) = lines[*index].trim().strip_suffix(':') {
            block_for_label.insert(label.to_owned(), block);
        }
    }
    let mut successors = vec![Vec::<usize>::new(); label_indices.len()];
    for (block, start) in label_indices.iter().enumerate() {
        let end = label_indices.get(block + 1).copied().unwrap_or(text_end);
        let mut fallthrough = true;
        for line in &lines[*start + 1..end] {
            let Some(branch) = lowered_branch(line) else {
                continue;
            };
            if let Some(target) = branch.target.and_then(|target| block_for_label.get(target))
                && !successors[block].contains(target)
            {
                successors[block].push(*target);
            }
            if branch.terminal && !branch.conditional {
                fallthrough = false;
                break;
            }
        }
        if fallthrough && block + 1 < label_indices.len() {
            successors[block].push(block + 1);
        }
    }

    let mut reachable = HashSet::new();
    let mut work = roots
        .iter()
        .filter_map(|root| block_for_label.get(*root).copied())
        .collect::<Vec<_>>();
    if let Some(start) = block_for_label.get("__ezra_start").copied() {
        work.push(start);
    }
    while let Some(block) = work.pop() {
        if !reachable.insert(block) {
            continue;
        }
        work.extend(successors[block].iter().copied());
    }

    let mut remove = vec![false; lines.len()];
    for (block, start) in label_indices.iter().enumerate() {
        if reachable.contains(&block) {
            continue;
        }
        let end = label_indices.get(block + 1).copied().unwrap_or(text_end);
        remove[*start..end].fill(true);
    }
    lines = lines
        .into_iter()
        .enumerate()
        .filter_map(|(index, line)| (!remove[index]).then_some(line))
        .collect();
    let new_text_start = lines
        .iter()
        .position(|line| line.trim() == "section .text")
        .unwrap_or(text_start.min(lines.len()));
    let new_text_end = lines[new_text_start + 1..]
        .iter()
        .position(|line| line.trim_start().starts_with("section "))
        .map_or(lines.len(), |offset| new_text_start + 1 + offset);
    simplify_lowered_branches(&lines, new_text_start, new_text_end)
}

#[derive(Clone, Copy)]
struct LoweredBranch<'a> {
    target: Option<&'a str>,
    conditional: bool,
    terminal: bool,
}

fn lowered_branch(line: &str) -> Option<LoweredBranch<'_>> {
    let trimmed = line.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let mnemonic = parts.next()?;
    let operands = parts.next().unwrap_or_default().trim();
    match mnemonic {
        "jp" | "jr" | "jmp" | "jz" | "jnz" | "jc" | "jnc" | "jm" | "jpe" | "jpo" => {
            if operands.starts_with('(') {
                return Some(LoweredBranch {
                    target: None,
                    conditional: false,
                    terminal: true,
                });
            }
            let conditional = !matches!(mnemonic, "jp" | "jr" | "jmp") || operands.contains(',');
            Some(LoweredBranch {
                target: operands.rsplit(',').next().map(str::trim),
                conditional,
                terminal: true,
            })
        }
        "call" => Some(LoweredBranch {
            target: operands.rsplit(',').next().map(str::trim),
            conditional: true,
            terminal: false,
        }),
        "ret" | "reti" | "retn" | "rz" | "rnz" | "rc" | "rnc" | "rp" | "rm" | "rpe" | "rpo"
        | "halt" => Some(LoweredBranch {
            target: None,
            conditional: !matches!(mnemonic, "ret" | "reti" | "retn" | "halt"),
            terminal: true,
        }),
        _ => None,
    }
}

fn simplify_lowered_branches(lines: &[&str], text_start: usize, text_end: usize) -> String {
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index >= text_start && index < text_end {
            let next = lines[index + 1..text_end].iter().find_map(|candidate| {
                let trimmed = candidate.trim();
                (!trimmed.is_empty() && !trimmed.starts_with(';')).then_some(trimmed)
            });
            if let (Some(branch), Some(next)) = (lowered_branch(line), next)
                && matches!(
                    line.split_whitespace().next(),
                    Some("jp" | "jr" | "jmp" | "jz" | "jnz" | "jc" | "jnc" | "jm" | "jpe" | "jpo")
                )
                && branch
                    .target
                    .is_some_and(|target| next == format!("{target}:"))
            {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[allow(dead_code)]
fn strip_unreachable_runtime_helpers(assembly: &str) -> String {
    const RUNTIME_LABELS: [&str; 16] = [
        "__ezra_pass",
        "__ezra_fail",
        "__ezra_memcpy",
        "__ezra_memset",
        "__ezra_mul_u8",
        "__ezra_mul_u16",
        "__ezra_mul_u24",
        "__ezra_mul_i24",
        "__ezra_div_u8",
        "__ezra_div_u16",
        "__ezra_div_u24",
        "__ezra_div_i24",
        "__ezra_mod_u8",
        "__ezra_mod_u16",
        "__ezra_mod_u24",
        "__ezra_mod_i24",
    ];

    let lines = assembly.lines().collect::<Vec<_>>();
    let mut blocks = HashMap::<&str, (usize, usize)>::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(label) = line.trim().strip_suffix(':') else {
            continue;
        };
        if !RUNTIME_LABELS.contains(&label) {
            continue;
        }
        let end = lines[index + 1..]
            .iter()
            .position(|candidate| {
                let trimmed = candidate.trim();
                trimmed.starts_with("section ")
                    || (!candidate.starts_with(' ')
                        && !candidate.starts_with('\t')
                        && !trimmed.starts_with('.')
                        && trimmed.ends_with(':'))
            })
            .map_or(lines.len(), |offset| index + 1 + offset);
        blocks.insert(label, (index, end));
    }

    if blocks.is_empty() {
        return assembly.to_owned();
    }

    let is_in_runtime_block = |index: usize| {
        blocks
            .values()
            .any(|(start, end)| (*start..*end).contains(&index))
    };
    let references = |label: &str, source: &[&str]| {
        source.iter().enumerate().any(|(index, line)| {
            !is_in_runtime_block(index)
                && (line.contains(&format!("call {label}"))
                    || line.contains(&format!("jp {label}")))
        })
    };

    let mut reachable = RUNTIME_LABELS
        .iter()
        .copied()
        .filter(|label| references(label, &lines))
        .collect::<HashSet<_>>();
    let mut changed = true;
    while changed {
        changed = false;
        for label in reachable.clone() {
            let Some((start, end)) = blocks.get(label).copied() else {
                continue;
            };
            for dependency in RUNTIME_LABELS {
                if !reachable.contains(dependency)
                    && lines[start..end].iter().any(|line| {
                        line.contains(&format!("call {dependency}"))
                            || line.contains(&format!("jp {dependency}"))
                    })
                {
                    reachable.insert(dependency);
                    changed = true;
                }
            }
        }
    }

    let mut output = String::new();
    for (index, line) in lines.iter().enumerate() {
        if blocks.iter().any(|(label, (start, end))| {
            !reachable.contains(label) && (*start..*end).contains(&index)
        }) {
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    output
}

fn is_integer_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Char(_) | Expr::Bool(_)
    )
}

fn is_immediate_u8(expr: &Expr) -> bool {
    is_integer_literal(expr)
}

fn is_unit_integer_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Int(1) | Expr::TypedInt(1, _))
}

fn is_self_unit_pointer_increment(name: &str, expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Binary {
            left,
            op: BinaryOp::Add,
            right,
        } if matches!(left.as_ref(), Expr::Ident(source) if source == name)
            && is_unit_integer_literal(right)
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoopLabels {
    continue_label: String,
    break_label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalConstant {
    value: i64,
    ty: Type,
}

struct Emitter {
    symbols: Symbols,
    out: String,
    rodata: String,
    label_counter: usize,
    scopes: Vec<HashMap<String, Variable>>,
    scope_types: Vec<HashMap<String, Type>>,
    local_constants: Vec<HashMap<String, LocalConstant>>,
    readonly_pointer_aliases: Vec<HashMap<String, u32>>,
    string_literals: HashMap<String, Variable>,
    emitted_string_literals: HashSet<String>,
    loop_stack: Vec<LoopLabels>,
    return_type_stack: Vec<Option<Type>>,
    second_return_type_stack: Vec<Option<Type>>,
    return_value_stack: Vec<bool>,
    second_return_pointer_stack: Vec<Option<Variable>>,
    function_name_stack: Vec<String>,
    function_frame_stack: Vec<bool>,
    function_interrupt_stack: Vec<bool>,
    function_naked_stack: Vec<bool>,
    function_storage_stack: Vec<Vec<Variable>>,
    function_local_plans: Vec<HashMap<String, Variable>>,
    cacheable_ranges: Vec<(u32, u32)>,
    assigned_names_stack: Vec<HashSet<String>>,
    storage_required_names_stack: Vec<HashSet<String>>,
    recursive_call_edges: HashSet<(String, String)>,
    tail_call_edges: HashSet<(String, String)>,
    static_liveness: Option<StaticLiveness>,
    required_runtime_helpers: HashSet<RuntimeHelper>,
    function_pointer_constants: HashSet<String>,

    cpu: CpuFamily,
    mos_executable: bool,
    ti_os_executable: bool,
    stack_top: Address24,
    rodata_base: Address24,
    eliminate_dead_code: bool,
}

impl Emitter {
    fn new(
        symbols: Symbols,
        options: AssemblyOptions,
        recursive_call_edges: HashSet<(String, String)>,
        tail_call_edges: HashSet<(String, String)>,
        static_liveness: Option<StaticLiveness>,
    ) -> Self {
        let string_literals = symbols.string_literals.clone();
        let cacheable_ranges = symbols
            .globals
            .values()
            .map(|variable| (variable.addr, variable.size))
            .collect();
        Self {
            symbols,
            out: String::new(),
            rodata: String::new(),
            label_counter: 0,
            scopes: Vec::new(),
            scope_types: Vec::new(),
            local_constants: Vec::new(),
            readonly_pointer_aliases: Vec::new(),
            string_literals,
            emitted_string_literals: HashSet::new(),
            loop_stack: Vec::new(),
            return_type_stack: Vec::new(),
            second_return_type_stack: Vec::new(),
            return_value_stack: Vec::new(),
            second_return_pointer_stack: Vec::new(),
            function_name_stack: Vec::new(),
            function_frame_stack: Vec::new(),
            function_interrupt_stack: Vec::new(),
            function_naked_stack: Vec::new(),
            function_storage_stack: Vec::new(),
            function_local_plans: Vec::new(),
            cacheable_ranges,
            assigned_names_stack: Vec::new(),
            storage_required_names_stack: Vec::new(),
            recursive_call_edges,
            tail_call_edges,
            static_liveness,
            required_runtime_helpers: HashSet::new(),
            function_pointer_constants: HashSet::new(),

            cpu: options.cpu,
            mos_executable: options.mos_executable,
            ti_os_executable: options.ti_os_executable,
            stack_top: options.stack_top,
            rodata_base: options.rodata_base,
            eliminate_dead_code: true,
        }
    }

    fn disable_dead_code_elimination(&mut self) {
        self.eliminate_dead_code = false;
    }

    fn emit_prelude(&mut self) {
        self.line("; generated by ezrac");
        match self.cpu {
            CpuFamily::Ez80 => self.line("; target: eZ80 ADL mode"),
            CpuFamily::Z80 => self.line("; target: Z80"),
            CpuFamily::R800 => self.line("; target: R800"),
            other => self.line(&format!("; target: {}", other.as_str())),
        }
        self.line("section .text");
        self.line("__ezra_start:");
        if self.mos_executable {
            self.line("    ei");
            return;
        }
        if self.ti_os_executable {
            return;
        }
        self.line("    di");
        if is_z80_family_16bit(self.cpu) {
            self.line(&format!(
                "    ld sp, {:04X}h",
                self.stack_top.get() & 0xFFFF
            ));
        } else {
            self.line(&format!("    ld sp, {:06X}h", self.stack_top.get()));
        }
    }

    fn alloc_var<S: Into<u32>>(&mut self, size: S) -> Variable {
        let variable = self.symbols.alloc_var(size);
        self.track_function_storage(variable);
        variable
    }

    fn alloc_storage(&mut self, ty: &Type) -> Result<Variable, Diagnostic> {
        let variable = self.symbols.alloc_storage(ty)?;
        self.track_function_storage(variable);
        Ok(variable)
    }

    fn track_function_storage(&mut self, variable: Variable) {
        self.cacheable_ranges.push((variable.addr, variable.size));
        if let Some(storage) = self.function_storage_stack.last_mut() {
            storage.push(variable);
        }
    }

    fn emit_required_sections(&mut self) {
        self.require_runtime_helpers_from_output();
        self.emit_runtime_helpers();
        let literals = self
            .string_literals
            .iter()
            .filter(|(value, _)| {
                self.static_liveness
                    .as_ref()
                    .is_none_or(|liveness| liveness.string_literals.contains(*value))
            })
            .map(|(value, variable)| (value.clone(), *variable))
            .collect::<Vec<_>>();
        for (value, variable) in literals {
            self.emit_string_literal_initializer(&value, variable);
        }
        self.line("section .header");
        self.line("section .rodata");
        if !self.rodata.is_empty() {
            self.line(&format!("org {:06X}h", self.rodata_base.get()));
            self.out.push_str(&self.rodata);
        }
        self.line("section .data");
        let mut function_pointer_constants = self
            .function_pointer_constants
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        function_pointer_constants.sort();
        for target in function_pointer_constants {
            self.line(&format!("{}:", function_pointer_constant_label(&target)));
            self.line(&format!("    dw {target}"));
        }
        for section in [".bss", ".assets", ".scratch"] {
            self.line(&format!("section {section}"));
        }
    }

    fn emit_start_tail(&mut self) {
        self.line("    call _main");
        self.line("__ezra_exit:");
        if self.mos_executable {
            self.line("    ld hl, 000000h");
            self.line("    ret");
            self.line("");
            return;
        }
        if self.ti_os_executable {
            self.line("    ret");
        } else {
            self.line("    jp __ezra_exit");
        }
        self.line("");
    }

    fn require_runtime_helper(&mut self, helper: RuntimeHelper) {
        if !self.required_runtime_helpers.insert(helper) {
            return;
        }
        for dependency in helper.dependencies() {
            self.require_runtime_helper(*dependency);
        }
    }

    fn require_runtime_helpers_from_output(&mut self) {
        let output = self.out.clone();
        for helper in RUNTIME_HELPER_ORDER {
            if output.lines().any(|line| {
                line.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                    .any(|token| token == helper.label())
            }) {
                self.require_runtime_helper(helper);
            }
        }
    }

    fn emit_runtime_helpers(&mut self) {
        if self.required_runtime_helpers.contains(&RuntimeHelper::Pass) {
            self.line("__ezra_pass:");
            self.emit_out(0x0D, 0);
            self.emit_out(0x0E, 1);
            self.line("    ret");
        }
        if self.required_runtime_helpers.contains(&RuntimeHelper::Fail) {
            self.line("__ezra_fail:");
            self.emit_out_a(0x0D);
            self.emit_out(0x0E, 1);
            self.line("    ret");
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::Memcpy)
        {
            self.line("__ezra_memcpy:");
            if is_intel_8080_family(self.cpu) {
                self.line("    mov a, b");
                self.line("    ora c");
                self.line("    rz");
                self.line(".L_memcpy_loop:");
                self.line("    mov a, m");
                self.line("    stax d");
                self.line("    inx h");
                self.line("    inx d");
                self.line("    dcx b");
                self.line("    mov a, b");
                self.line("    ora c");
                self.line("    jnz .L_memcpy_loop");
                self.line("    ret");
            } else {
                self.line("    push de");
                self.line("    push hl");
                self.line("    push bc");
                self.line("    pop hl");
                self.line("    ld de, 000000h");
                self.line("    or a");
                self.line("    sbc hl, de");
                self.line("    pop hl");
                self.line("    pop de");
                self.line("    ret z");
                self.line("    ex de, hl");
                self.line("    ldir");
                self.line("    ret");
            }
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::Memset)
        {
            self.line("__ezra_memset:");
            if is_intel_8080_family(self.cpu) {
                self.line("    mov e, a");
                self.line("    mov a, b");
                self.line("    ora c");
                self.line("    rz");
                self.line("    mov a, e");
                self.line(".L_memset_loop:");
                self.line("    mov m, a");
                self.line("    inx h");
                self.line("    dcx b");
                self.line("    mov d, a");
                self.line("    mov a, b");
                self.line("    ora c");
                self.line("    mov a, d");
                self.line("    jnz .L_memset_loop");
                self.line("    ret");
            } else {
                self.line("    push hl");
                self.line("    push bc");
                self.line("    pop hl");
                self.line("    ld de, 000000h");
                self.line("    or a");
                self.line("    sbc hl, de");
                self.line("    pop hl");
                self.line("    ret z");
                self.line("    ld (hl), a");
                self.line("    dec bc");
                self.line("    push hl");
                self.line("    push bc");
                self.line("    pop hl");
                self.line("    ld de, 000000h");
                self.line("    or a");
                self.line("    sbc hl, de");
                self.line("    pop hl");
                self.line("    ret z");
                self.line("    push hl");
                self.line("    inc hl");
                self.line("    ex de, hl");
                self.line("    pop hl");
                self.line("    ldir");
                self.line("    ret");
            }
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::MulU8)
        {
            self.line("__ezra_mul_u8:");
            if self.cpu == CpuFamily::R800 {
                self.line("    mulub a, c");
                self.line("    ld a, l");
                self.line("    ret");
            } else if is_z80_family_16bit(self.cpu) {
                self.line("    ld b, a");
                self.line("    xor a");
                self.line(".L_mul_u8_loop:");
                self.line("    ld d, a");
                self.line("    ld a, b");
                self.line("    or b");
                self.line("    ld a, d");
                self.line("    ret z");
                self.line("    add a, c");
                self.line("    dec b");
                self.line("    jp .L_mul_u8_loop");
            } else {
                self.line("    ld b, a");
                self.line("    mlt bc");
                self.line("    ld a, c");
                self.line("    ret");
            }
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::MulU16)
        {
            self.line("__ezra_mul_u16:");
            if self.cpu == CpuFamily::R800 {
                self.line("    muluw hl, bc");
                self.line("    ret");
            } else if is_z80_family_16bit(self.cpu) {
                self.line("    ld de, 0000h");
                self.line(".L_mul_u16_loop:");
                self.line("    ld a, b");
                self.line("    or c");
                self.line("    jp z, .L_mul_u16_done");
                self.line("    ex de, hl");
                self.line("    add hl, de");
                self.line("    ex de, hl");
                self.line("    dec bc");
                self.line("    jp .L_mul_u16_loop");
                self.line(".L_mul_u16_done:");
                self.line("    ex de, hl");
                self.line("    ret");
            } else {
                self.line("    ld d, h");
                self.line("    ld e, l");
                self.line("    ld h, c");
                self.line("    mlt hl");
                self.line("    push hl");
                self.line("    ld h, d");
                self.line("    ld l, c");
                self.line("    mlt hl");
                self.line("    ld a, l");
                self.line("    ld h, e");
                self.line("    ld l, b");
                self.line("    mlt hl");
                self.line("    add a, l");
                self.line("    pop de");
                self.line("    add a, d");
                self.line("    ld hl, 000000h");
                self.line("    ld h, a");
                self.line("    ld l, e");
                self.line("    ret");
            }
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::MulU24)
        {
            self.line("__ezra_mul_u24:");
            self.line("    ex de, hl");
            self.line("    ld hl, 000000h");
            self.line(".L_mul_u24_loop:");
            self.line("    push hl");
            self.line("    ld hl, 000000h");
            self.line("    or a");
            self.line("    sbc hl, bc");
            self.line("    jp z, .L_mul_u24_done");
            self.line("    pop hl");
            self.line("    add hl, de");
            self.line("    dec bc");
            self.line("    jp .L_mul_u24_loop");
            self.line(".L_mul_u24_done:");
            self.line("    pop hl");
            self.line("    ret");
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::MulI24)
        {
            self.line("__ezra_mul_i24:");
            self.line("    jp __ezra_mul_u24");
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::DivU8)
        {
            self.line("__ezra_div_u8:");
            self.line("    ld d, a");
            self.line("    xor a");
            self.line("    ld b, a");
            self.line("    ld a, c");
            self.line("    or a");
            self.line("    jp z, .L_div_u8_zero");
            self.line(".L_div_u8_loop:");
            self.line("    ld a, d");
            self.line("    cp c");
            self.line("    jp c, .L_div_u8_done");
            self.line("    sub c");
            self.line("    ld d, a");
            self.line("    inc b");
            self.line("    jp .L_div_u8_loop");
            self.line(".L_div_u8_zero:");
            self.line("    xor a");
            self.line("    ret");
            self.line(".L_div_u8_done:");
            self.line("    ld a, b");
            self.line("    ret");
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::DivU16)
        {
            self.line("__ezra_div_u16:");
            self.line("    ld a, b");
            self.line("    or c");
            self.line("    jp z, .L_div_u16_zero");
            self.line("    ex de, hl");
            self.line("    ld hl, 000000h");
            self.line(".L_div_u16_loop:");
            self.line("    push hl");
            self.line("    push de");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, bc");
            self.line("    jp c, .L_div_u16_done");
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.line("    inc hl");
            self.line("    jp .L_div_u16_loop");
            self.line(".L_div_u16_zero:");
            self.line("    ld hl, 000000h");
            self.line("    ret");
            self.line(".L_div_u16_done:");
            self.line("    pop hl");
            self.line("    ret");
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::DivU24)
        {
            self.line("__ezra_div_u24:");
            self.line("    push hl");
            self.line("    ld hl, 000000h");
            self.line("    or a");
            self.line("    sbc hl, bc");
            self.line("    jp z, .L_div_u24_zero");
            self.line("    pop de");
            self.line("    ld hl, 000000h");
            self.line(".L_div_u24_loop:");
            self.line("    push hl");
            self.line("    push de");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, bc");
            self.line("    jp c, .L_div_u24_done");
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.line("    inc hl");
            self.line("    jp .L_div_u24_loop");
            self.line(".L_div_u24_zero:");
            self.line("    pop hl");
            self.line("    ld hl, 000000h");
            self.line("    ret");
            self.line(".L_div_u24_done:");
            self.line("    pop hl");
            self.line("    ret");
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::ModU8)
        {
            self.line("__ezra_mod_u8:");
            self.line("    ld d, a");
            self.line("    ld a, c");
            self.line("    or a");
            self.line("    jp z, .L_mod_u8_zero");
            self.line(".L_mod_u8_loop:");
            self.line("    ld a, d");
            self.line("    cp c");
            self.line("    jp c, .L_mod_u8_done");
            self.line("    sub c");
            self.line("    ld d, a");
            self.line("    jp .L_mod_u8_loop");
            self.line(".L_mod_u8_zero:");
            self.line("    xor a");
            self.line("    ret");
            self.line(".L_mod_u8_done:");
            self.line("    ld a, d");
            self.line("    ret");
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::ModU16)
        {
            self.line("__ezra_mod_u16:");
            self.line("    ld a, b");
            self.line("    or c");
            self.line("    jp z, .L_mod_u16_zero");
            self.line("    ex de, hl");
            self.line(".L_mod_u16_loop:");
            self.line("    push de");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, bc");
            self.line("    jp c, .L_mod_u16_done");
            self.line("    ex de, hl");
            self.line("    jp .L_mod_u16_loop");
            self.line(".L_mod_u16_zero:");
            self.line("    ld hl, 000000h");
            self.line("    ret");
            self.line(".L_mod_u16_done:");
            self.line("    push de");
            self.line("    pop hl");
            self.line("    ret");
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::ModU24)
        {
            self.line("__ezra_mod_u24:");
            self.line("    push hl");
            self.line("    ld hl, 000000h");
            self.line("    or a");
            self.line("    sbc hl, bc");
            self.line("    jp z, .L_mod_u24_zero");
            self.line("    pop de");
            self.line(".L_mod_u24_loop:");
            self.line("    push de");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, bc");
            self.line("    jp c, .L_mod_u24_done");
            self.line("    ex de, hl");
            self.line("    jp .L_mod_u24_loop");
            self.line(".L_mod_u24_zero:");
            self.line("    pop hl");
            self.line("    ld hl, 000000h");
            self.line("    ret");
            self.line(".L_mod_u24_done:");
            self.line("    push de");
            self.line("    pop hl");
            self.line("    ret");
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::DivI24)
        {
            self.emit_signed_i24_div_mod_helper("__ezra_div_i24", BinaryOp::Div);
        }
        if self
            .required_runtime_helpers
            .contains(&RuntimeHelper::ModI24)
        {
            self.emit_signed_i24_div_mod_helper("__ezra_mod_i24", BinaryOp::Mod);
        }
    }

    fn emit_signed_i24_div_mod_helper(&mut self, label: &str, op: BinaryOp) {
        let dividend = self.alloc_var(ValueWidth::U24.bytes());
        let divisor = self.alloc_var(ValueWidth::U24.bytes());
        let quotient = self.alloc_var(ValueWidth::U24.bytes());
        let quotient_negative = self.alloc_var(ValueWidth::U8.bytes());
        let remainder_negative = self.alloc_var(ValueWidth::U8.bytes());
        let loop_label = self.next_label("sdiv_i24_loop");
        let zero_label = self.next_label("sdiv_i24_zero");
        let done_label = self.next_label("sdiv_i24_done");
        let quotient_positive_label = self.next_label("sdiv_i24_q_positive");
        let remainder_positive_label = self.next_label("sdiv_i24_r_positive");
        let not_overflow_label = self.next_label("sdiv_i24_not_overflow");

        self.line(&format!("{label}:"));
        self.emit_store_width(dividend);
        self.line("    push bc");
        self.line("    pop hl");
        self.emit_store_width(divisor);
        self.emit_zero_variable(quotient);
        self.emit_zero_variable(quotient_negative);
        self.emit_zero_variable(remainder_negative);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_jump_if_memory_not_equals(
            dividend,
            signed_min_bytes(ValueWidth::U24),
            &not_overflow_label,
        );
        self.emit_jump_if_memory_not_equals(
            divisor,
            signed_negative_one_bytes(ValueWidth::U24),
            &not_overflow_label,
        );
        match op {
            BinaryOp::Div => self.emit_load_width(dividend),
            BinaryOp::Mod => {
                self.emit_zero_variable(dividend);
                self.emit_load_width(dividend);
            }
            _ => unreachable!("not a division op"),
        }
        self.line("    ret");
        self.line(&format!("{not_overflow_label}:"));

        self.emit_abs_signed_variable(dividend, Some(quotient_negative), Some(remainder_negative));
        self.emit_abs_signed_variable(divisor, Some(quotient_negative), None);

        self.line(&format!("{loop_label}:"));
        self.emit_load_width(dividend);
        self.line("    push hl");
        self.emit_load_width(divisor);
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line(&format!("    jp c, {done_label}"));
        self.emit_store_width(dividend);
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        self.emit_load_a(quotient_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {quotient_positive_label}"));
        self.emit_negate_memory(quotient);
        self.line(&format!("{quotient_positive_label}:"));
        self.emit_load_a(remainder_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {remainder_positive_label}"));
        self.emit_negate_memory(dividend);
        self.line(&format!("{remainder_positive_label}:"));

        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        self.line("    ret");
    }

    fn emit_global_initializers(&mut self, program: &Program) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            match unwrapped_declaration(declaration) {
                Declaration::Global(decl)
                    if self
                        .static_liveness
                        .as_ref()
                        .is_none_or(|liveness| liveness.globals.contains(&decl.name)) =>
                {
                    if let Some(variable) = self.symbols.globals.get(&decl.name).copied() {
                        self.emit_storage_initializer(variable, &decl.ty, &decl.value)?;
                    }
                }
                Declaration::Const(decl)
                    if matches!(decl.ty, Type::Array { .. })
                        && self
                            .static_liveness
                            .as_ref()
                            .is_none_or(|liveness| liveness.constants.contains(&decl.name)) =>
                {
                    if let Some(variable) = self.symbols.globals.get(&decl.name).copied() {
                        self.emit_storage_initializer(variable, &decl.ty, &decl.value)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn emit_embed_initializers(&mut self) {
        let embeds = self
            .symbols
            .embeds
            .iter()
            .filter(|(name, _)| {
                self.static_liveness
                    .as_ref()
                    .is_none_or(|liveness| liveness.embeds.contains(*name))
            })
            .map(|(_, embed)| embed.clone())
            .collect::<Vec<_>>();
        for embed in embeds {
            for (offset, byte) in embed.bytes.into_iter().enumerate() {
                self.line(&format!("    ld a, {byte:02X}h"));
                self.emit_store_a(scalar_var(
                    embed.variable.addr + offset as u32,
                    u32::from(ValueWidth::U8.bytes()),
                ));
            }
        }
    }

    fn emit_string_literal_initializers(&mut self) {
        let mut literals = self
            .string_literals
            .iter()
            .chain(self.symbols.string_literals.iter())
            .filter(|(value, _)| {
                self.static_liveness
                    .as_ref()
                    .is_none_or(|liveness| liveness.string_literals.contains(*value))
            })
            .map(|(value, variable)| (variable.addr, value.clone(), *variable))
            .collect::<Vec<_>>();
        literals.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        literals.dedup_by(|left, right| left.1 == right.1);
        literals.sort_by_key(|(addr, _, _)| *addr);
        for (_, value, variable) in literals {
            self.emit_string_literal_initializer(&value, variable);
        }
    }

    fn emit_string_literal_initializer(&mut self, value: &str, _variable: Variable) {
        if self.emitted_string_literals.insert(value.to_owned()) {
            self.rodata
                .push_str(&terminated_text_data_line(".dm", value, "00h"));
            self.rodata.push('\n');
        }
    }

    fn emit_function_pointer_trampolines(
        &mut self,
        program: &Program,
        emitted_functions: &HashSet<String>,
    ) -> Result<(), Diagnostic> {
        for declaration in &program.declarations {
            let Declaration::Function(function) = unwrapped_declaration(declaration) else {
                continue;
            };
            if !emitted_functions.contains(&function.name) {
                continue;
            }
            let Some(signature) = self.symbols.functions.get(&function.name).cloned() else {
                continue;
            };
            if !signature.uses_arg_slots || signature.second_return_type.is_some() {
                continue;
            }
            let slots = self.symbols.function_pointer_arg_slots(
                &signature.param_types,
                signature.return_type.as_ref(),
            )?;
            self.line(&format!("{}:", function_pointer_label(&function.name)));
            for (source, target) in slots
                .iter()
                .copied()
                .zip(signature.arg_slots.iter().copied())
            {
                self.emit_load_width(source);
                self.emit_store_width(target);
            }
            self.line(&format!("    call {}", function_label(&function.name)));
            self.line("    ret");
        }
        Ok(())
    }

    fn emit_function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        validate_function_attrs(function)?;
        let naked = has_attr(function, "naked");
        let interrupt = has_attr(function, "interrupt");
        if interrupt {
            if !function.params.is_empty() {
                return Err(Diagnostic::new(format!(
                    "interrupt function `{}` cannot take parameters",
                    function.name
                )));
            }
            if function.return_type.is_some() || function.second_return_type.is_some() {
                return Err(Diagnostic::new(format!(
                    "interrupt function `{}` cannot return a value",
                    function.name
                )));
            }
        }
        if function.second_return_type.is_some() && function.return_type.is_none() {
            return Err(Diagnostic::new(format!(
                "two-result function `{}` must have a first return type",
                function.name
            )));
        }
        if naked && function.second_return_type.is_some() {
            return Err(Diagnostic::new(format!(
                "naked two-result function `{}` is not supported",
                function.name
            )));
        }
        if naked {
            for stmt in &function.body {
                let Stmt::Asm {
                    inputs, outputs, ..
                } = stmt
                else {
                    return Err(Diagnostic::new(format!(
                        "naked function `{}` may contain only asm blocks",
                        function.name
                    )));
                };
                if !inputs.is_empty() || !outputs.is_empty() {
                    return Err(Diagnostic::new(format!(
                        "naked function `{}` asm blocks cannot use operands",
                        function.name
                    )));
                }
            }
        }
        if !naked && function.second_return_type.is_some() {
            if !block_guarantees_two_value_return(&function.body, &self.symbols) {
                return Err(Diagnostic::new(format!(
                    "missing two return values in function `{}`",
                    function.name
                )));
            }
        } else if !naked
            && function.return_type.is_some()
            && !block_guarantees_value_return(&function.body, &self.symbols)
        {
            return Err(Diagnostic::new(format!(
                "missing return value in function `{}`",
                function.name
            )));
        }
        self.line(&format!("{}:", function_label(&function.name)));
        self.scopes.push(HashMap::new());
        self.scope_types.push(HashMap::new());
        self.local_constants.push(HashMap::new());
        self.readonly_pointer_aliases.push(HashMap::new());
        self.assigned_names_stack
            .push(assigned_names_in_block(&function.body));
        self.storage_required_names_stack
            .push(storage_required_names_in_block(&function.body));
        if let Some(return_type) = &function.return_type {
            self.symbols.type_width(return_type)?;
        }
        if let Some(second_return_type) = &function.second_return_type {
            self.symbols.type_width(second_return_type)?;
        }
        let uses_stack_frame = self
            .symbols
            .functions
            .get(&function.name)
            .is_some_and(|sig| sig.stack_arg_bytes > 0);
        self.return_type_stack.push(function.return_type.clone());
        self.second_return_type_stack
            .push(function.second_return_type.clone());
        self.return_value_stack.push(function.return_type.is_some());
        self.second_return_pointer_stack.push(None);
        self.function_name_stack.push(function.name.clone());
        self.function_frame_stack.push(uses_stack_frame);
        self.function_interrupt_stack.push(interrupt);
        self.function_naked_stack.push(naked);
        self.function_storage_stack.push(Vec::new());
        if !naked {
            if interrupt {
                self.emit_interrupt_prologue();
            }
            if uses_stack_frame {
                self.emit_frame_prologue();
            }
            self.bind_params(function)?;
        }
        let local_plan = self.plan_function_locals(function)?;
        self.function_local_plans.push(local_plan);
        self.emit_block(&function.body)?;
        self.function_local_plans.pop();
        self.function_naked_stack.pop();
        self.function_interrupt_stack.pop();
        self.function_frame_stack.pop();
        self.function_name_stack.pop();
        self.function_storage_stack.pop();
        self.return_value_stack.pop();
        self.second_return_pointer_stack.pop();
        self.second_return_type_stack.pop();
        self.return_type_stack.pop();
        self.storage_required_names_stack.pop();
        self.assigned_names_stack.pop();
        self.readonly_pointer_aliases.pop();
        self.local_constants.pop();
        self.scope_types.pop();
        self.scopes.pop();
        if naked {
            return Ok(());
        }
        if interrupt {
            self.emit_interrupt_epilogue();
            return Ok(());
        }
        if function.name == "main" {
            self.line("    jp __ezra_exit");
        } else {
            if uses_stack_frame {
                self.emit_frame_epilogue();
            }
            self.line("    ret");
        }
        Ok(())
    }

    fn plan_function_locals(
        &mut self,
        function: &Function,
    ) -> Result<HashMap<String, Variable>, Diagnostic> {
        let saved_scope = self.current_scope_mut().clone();
        let saved_types = self.current_scope_types_mut().clone();
        let saved_constants = self.current_local_constants_mut().clone();
        let saved_aliases = self.current_readonly_pointer_aliases_mut().clone();

        let result = (|| {
            let mut locals = Vec::new();
            let mut local_types = HashMap::new();
            let mut asm_output_names = HashSet::new();
            let mut asm_clobbers_memory = false;
            collect_inline_asm_storage_requirements(
                &function.body,
                &mut asm_output_names,
                &mut asm_clobbers_memory,
            );
            self.collect_function_locals(
                &function.body,
                &mut locals,
                &mut local_types,
                &asm_output_names,
                asm_clobbers_memory,
            )?;

            let target = ez80_local_target(self.cpu);
            let clobbers = (0..target.registers.len()).map(PhysReg).collect::<Vec<_>>();
            let planned = allocate_source_locals(&target, &locals, &function.body, &clobbers)
                .map_err(|diagnostics| {
                    let details = diagnostics
                        .iter()
                        .map(|diagnostic| match diagnostic.message.as_str() {
                            "break appears outside a loop" => "`break` outside loop".to_owned(),
                            "continue appears outside a loop" => {
                                "`continue` outside loop".to_owned()
                            }
                            _ => diagnostic.to_string(),
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    if diagnostics.len() == 1
                        && matches!(
                            diagnostics[0].message.as_str(),
                            "break appears outside a loop" | "continue appears outside a loop"
                        )
                    {
                        Diagnostic::new(details)
                    } else {
                        Diagnostic::new(format!(
                            "eZ80 local allocation failed in function `{}`: {details}",
                            function.name,
                        ))
                    }
                })?;
            let storage_size = planned
                .allocation
                .spill_slots
                .iter()
                .map(|slot| slot.offset.saturating_add(slot.size))
                .max()
                .unwrap_or(0);
            let storage = (storage_size != 0).then(|| self.alloc_var(storage_size));
            let mut variables = HashMap::new();

            for local in &locals {
                let vreg = planned.locals.vreg(&local.name).ok_or_else(|| {
                    Diagnostic::new(format!("missing allocation for local `{}`", local.name))
                })?;
                let slot_index = match planned.allocation.location(vreg) {
                    Some(Location::Spill(slot)) => slot,
                    Some(Location::Register(_)) => {
                        return Err(Diagnostic::new(format!(
                            "memory-only local `{}` was allocated to a register",
                            local.name
                        )));
                    }
                    Some(Location::Unused) | None => {
                        return Err(Diagnostic::new(format!(
                            "source allocator did not place local `{}`",
                            local.name
                        )));
                    }
                };
                let slot = planned
                    .allocation
                    .spill_slots
                    .get(slot_index)
                    .ok_or_else(|| Diagnostic::new("invalid eZ80 static spill slot"))?;
                debug_assert_eq!(slot.class, EZ80_STATIC_SPILL_CLASS);
                let base = storage.ok_or_else(|| {
                    Diagnostic::new("eZ80 local allocation omitted its static storage region")
                })?;
                let ty = local_types.get(&local.name).ok_or_else(|| {
                    Diagnostic::new(format!("missing type for local `{}`", local.name))
                })?;
                variables.insert(
                    local.name.clone(),
                    self.symbols.storage_at(base.addr + slot.offset, ty)?,
                );
            }
            Ok(variables)
        })();

        *self.current_scope_mut() = saved_scope;
        *self.current_scope_types_mut() = saved_types;
        *self.current_local_constants_mut() = saved_constants;
        *self.current_readonly_pointer_aliases_mut() = saved_aliases;
        result
    }

    fn collect_function_locals(
        &mut self,
        body: &[Stmt],
        locals: &mut Vec<SourceLocal>,
        local_types: &mut HashMap<String, Type>,
        asm_output_names: &HashSet<String>,
        asm_clobbers_memory: bool,
    ) -> Result<(), Diagnostic> {
        for stmt in body {
            match stmt {
                Stmt::Let { name, ty, value } => {
                    if self.name_in_current_function(name) {
                        return Err(Diagnostic::new(format!(
                            "local `{name}` shadows an existing name"
                        )));
                    }
                    self.current_scope_types_mut()
                        .insert(name.clone(), ty.clone());
                    // Validate before deciding that a constant local needs no
                    // storage. The optimized program may remove this let, but
                    // invalid source must still be rejected.
                    self.validate_expr_arithmetic_compatibility(value)?;
                    self.validate_expr_assignable_to_type(value, ty)?;
                    if asm_clobbers_memory
                        || asm_output_names.contains(name)
                        || !self.can_elide_constant_local_storage(name, ty, value)?
                    {
                        let size = self.symbols.type_size(ty)?;
                        locals.push(
                            SourceLocal::new(name.clone(), size, 1, EZ80_MEMORY_LOCAL_CLASS)
                                .with_spill_classes(vec![EZ80_STATIC_SPILL_CLASS])
                                .with_force_memory(true),
                        );
                        local_types.insert(name.clone(), ty.clone());
                        let placeholder = self.symbols.storage_at(0, ty)?;
                        self.current_scope_mut().insert(name.clone(), placeholder);
                    }
                    self.record_local_constant(name, ty, value);
                    self.record_readonly_pointer_alias(name, value);
                }
                Stmt::LetTwo {
                    first_name,
                    first_ty,
                    second_name,
                    second_ty,
                    value,
                } => {
                    if self.name_in_current_function(first_name) {
                        return Err(Diagnostic::new(format!(
                            "local `{first_name}` shadows an existing name"
                        )));
                    }
                    self.current_scope_types_mut()
                        .insert(first_name.clone(), first_ty.clone());
                    if self.name_in_current_function(second_name) {
                        return Err(Diagnostic::new(format!(
                            "local `{second_name}` shadows an existing name"
                        )));
                    }
                    self.current_scope_types_mut()
                        .insert(second_name.clone(), second_ty.clone());
                    self.validate_two_result_value(value, first_ty, second_ty)?;
                    for (name, ty) in [(first_name, first_ty), (second_name, second_ty)] {
                        let size = self.symbols.type_size(ty)?;
                        locals.push(
                            SourceLocal::new(name.clone(), size, 1, EZ80_MEMORY_LOCAL_CLASS)
                                .with_spill_classes(vec![EZ80_STATIC_SPILL_CLASS])
                                .with_force_memory(true),
                        );
                        local_types.insert(name.clone(), ty.clone());
                        let placeholder = self.symbols.storage_at(0, ty)?;
                        self.current_scope_mut().insert(name.clone(), placeholder);
                    }
                }
                Stmt::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    self.collect_function_locals(
                        then_body,
                        locals,
                        local_types,
                        asm_output_names,
                        asm_clobbers_memory,
                    )?;
                    self.collect_function_locals(
                        else_body,
                        locals,
                        local_types,
                        asm_output_names,
                        asm_clobbers_memory,
                    )?;
                }
                Stmt::While { body, .. } | Stmt::Loop { body } => {
                    self.collect_function_locals(
                        body,
                        locals,
                        local_types,
                        asm_output_names,
                        asm_clobbers_memory,
                    )?;
                }
                Stmt::Asm {
                    outputs, clobbers, ..
                } => {
                    for output in outputs {
                        self.invalidate_local_constant(&output.name);
                        self.invalidate_readonly_pointer_alias(&output.name);
                    }
                    if asm_clobbers_include(clobbers, "memory") {
                        self.invalidate_all_local_constants();
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn emit_frame_prologue(&mut self) {
        if is_z80_family_16bit(self.cpu) {
            self.line("    push bc");
        } else {
            self.line("    push ix");
            self.line("    ld ix, 000000h");
            self.line("    add ix, sp");
        }
    }

    fn emit_frame_epilogue(&mut self) {
        if is_z80_family_16bit(self.cpu) {
            self.line("    pop bc");
        } else {
            self.line("    pop ix");
        }
    }

    fn emit_interrupt_prologue(&mut self) {
        self.line("    push af");
        self.line("    push bc");
        self.line("    push de");
        self.line("    push hl");
        self.line("    push ix");
        self.line("    push iy");
    }

    fn emit_interrupt_epilogue(&mut self) {
        self.line("    pop iy");
        self.line("    pop ix");
        self.line("    pop hl");
        self.line("    pop de");
        self.line("    pop bc");
        self.line("    pop af");
        self.line("    reti");
    }

    fn bind_params(&mut self, function: &Function) -> Result<(), Diagnostic> {
        let sig = self
            .symbols
            .functions
            .get(&function.name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{}`", function.name)))?;

        for (index, param) in function.params.iter().enumerate() {
            if self.name_in_current_function(&param.name) {
                return Err(Diagnostic::new(format!(
                    "parameter `{}` shadows an existing name",
                    param.name
                )));
            }
            let width = self.symbols.type_width(&param.ty)?;
            let variable = self.alloc_var(width.bytes());
            self.current_scope_mut()
                .insert(param.name.clone(), variable);
            self.current_scope_types_mut()
                .insert(param.name.clone(), param.ty.clone());
            if sig.uses_arg_slots {
                let slot = sig.arg_slots[index];
                self.emit_load_width(slot);
                self.emit_store_width(variable);
                continue;
            }
            if let Some(offset) = sig.stack_arg_offsets[index] {
                self.emit_load_ix_offset_width_into(offset, variable)?;
                continue;
            }
            match width {
                ValueWidth::U8 => {
                    match index {
                        0 => {}
                        1 => self.line("    ld a, b"),
                        2 => self.line("    ld a, c"),
                        _ => unreachable!("param count checked"),
                    }
                    self.emit_store_a(variable);
                }
                ValueWidth::U16 | ValueWidth::U24 => {
                    match index {
                        0 => {}
                        1 => self.line("    ex de, hl"),
                        2 => {
                            self.line("    push bc");
                            self.line("    pop hl");
                        }
                        _ => unreachable!("param count checked"),
                    }
                    self.emit_store_width(variable);
                }
            }
        }
        if let Some(offset) = sig.hidden_return_arg_offset {
            let pointer_width = self.symbols.type_width(&Type::Named("ptr".to_owned()))?;
            let pointer = self.alloc_var(pointer_width.bytes());
            self.emit_load_ix_offset_width_into(offset, pointer)?;
            if let Some(return_pointer) = self.second_return_pointer_stack.last_mut() {
                *return_pointer = Some(pointer);
            }
        }
        Ok(())
    }

    fn emit_block(&mut self, body: &[Stmt]) -> Result<(), Diagnostic> {
        for stmt in body {
            self.emit_stmt(stmt)?;
            if self.eliminate_dead_code && self.stmt_terminates_current_block(stmt) {
                break;
            }
        }
        Ok(())
    }

    fn block_terminates_current_block(&self, body: &[Stmt]) -> bool {
        body.iter()
            .any(|stmt| self.stmt_terminates_current_block(stmt))
    }

    fn stmt_terminates_current_block(&self, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Return(_) | Stmt::ReturnTwo { .. } | Stmt::Break | Stmt::Continue => true,
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                if let Ok(value) = self.eval_i64_with_local_constants(condition) {
                    if value == 0 {
                        return self.block_terminates_current_block(else_body);
                    }
                    return self.block_terminates_current_block(then_body);
                }
                !else_body.is_empty()
                    && self.block_terminates_current_block(then_body)
                    && self.block_terminates_current_block(else_body)
            }
            Stmt::Loop { body } => {
                !block_can_break_current_loop(body) && self.block_terminates_current_block(body)
            }
            Stmt::While { condition, body } => {
                self.eval_i64_with_local_constants(condition)
                    .is_ok_and(|value| value != 0)
                    && !block_can_break_current_loop(body)
                    && self.block_terminates_current_block(body)
            }
            _ => false,
        }
    }

    fn emit_stmt(&mut self, stmt: &Stmt) -> Result<(), Diagnostic> {
        self.line(&format!("    ; source: {}", stmt_summary(stmt)));
        match stmt {
            Stmt::Let { name, ty, value } => {
                if self.name_in_current_function(name) {
                    return Err(Diagnostic::new(format!(
                        "local `{name}` shadows an existing name"
                    )));
                }
                self.current_scope_types_mut()
                    .insert(name.clone(), ty.clone());
                if self.can_elide_constant_local_storage(name, ty, value)? {
                    self.record_local_constant(name, ty, value);
                    self.record_readonly_pointer_alias(name, value);
                    return Ok(());
                }
                let variable = self
                    .function_local_plans
                    .last()
                    .and_then(|locals| locals.get(name))
                    .copied()
                    .ok_or_else(|| {
                        Diagnostic::new(format!("missing storage allocation for local `{name}`"))
                    })?;
                self.current_scope_mut().insert(name.clone(), variable);
                self.emit_storage_initializer(variable, ty, value)?;
                self.record_local_constant(name, ty, value);
                self.record_readonly_pointer_alias(name, value);
            }
            Stmt::LetTwo {
                first_name,
                first_ty,
                second_name,
                second_ty,
                value,
            } => {
                if self.name_in_current_function(first_name) {
                    return Err(Diagnostic::new(format!(
                        "local `{first_name}` shadows an existing name"
                    )));
                }
                self.current_scope_types_mut()
                    .insert(first_name.clone(), first_ty.clone());
                if self.name_in_current_function(second_name) {
                    return Err(Diagnostic::new(format!(
                        "local `{second_name}` shadows an existing name"
                    )));
                }
                self.current_scope_types_mut()
                    .insert(second_name.clone(), second_ty.clone());
                self.validate_two_result_value(value, first_ty, second_ty)?;
                let first = self
                    .function_local_plans
                    .last()
                    .and_then(|locals| locals.get(first_name))
                    .copied()
                    .ok_or_else(|| {
                        Diagnostic::new(format!(
                            "missing storage allocation for local `{first_name}`"
                        ))
                    })?;
                let second = self
                    .function_local_plans
                    .last()
                    .and_then(|locals| locals.get(second_name))
                    .copied()
                    .ok_or_else(|| {
                        Diagnostic::new(format!(
                            "missing storage allocation for local `{second_name}`"
                        ))
                    })?;
                self.emit_two_result_value(value, second)?;
                self.current_scope_mut().insert(first_name.clone(), first);
                self.current_scope_mut().insert(second_name.clone(), second);
                self.emit_store_width(first);
            }
            Stmt::Assign { target, op, value } => {
                self.emit_assignment(target, *op, value)?;
            }
            Stmt::Out { port, value } => {
                let port = self.port(port)?;
                self.validate_expr_assignable_to_type(value, &Type::Named("u8".to_owned()))?;
                self.emit_expr_to_a(value)?;
                self.emit_out_a(port);
            }
            Stmt::Expr(Expr::Call { path, args }) => self.emit_call(path, args)?,
            Stmt::Expr(expr) => {
                let width = self.expr_width(expr)?;
                self.emit_expr_to_width(expr, width)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.ensure_expr_is_bool(condition, "if condition")?;
                if self.eliminate_dead_code
                    && let Ok(value) = self.eval_i64_with_local_constants(condition)
                {
                    if value == 0 {
                        self.emit_block(else_body)?;
                    } else {
                        self.emit_block(then_body)?;
                    }
                    return Ok(());
                }
                let else_label = self.next_label("else");
                let end_label = self.next_label("endif");
                if !self.emit_jump_if_false(condition, &else_label)? {
                    self.emit_expr_to_a(condition)?;
                    self.line("    or a");
                    self.line(&format!("    jp z, {else_label}"));
                }
                self.emit_block(then_body)?;
                if !self.block_terminates_current_block(then_body) {
                    self.line(&format!("    jp {end_label}"));
                }
                self.line(&format!("{else_label}:"));
                self.emit_block(else_body)?;
                self.line(&format!("{end_label}:"));
            }
            Stmt::While { condition, body } => {
                self.ensure_expr_is_bool(condition, "while condition")?;
                let mut condition_is_always_true = false;
                if self.eliminate_dead_code
                    && let Ok(value) = self.eval_i64_with_local_constants(condition)
                {
                    if value == 0 {
                        return Ok(());
                    }
                    condition_is_always_true = true;
                }
                let start_label = self.next_label("while");
                let end_label = self.next_label("endwhile");
                self.loop_stack.push(LoopLabels {
                    continue_label: start_label.clone(),
                    break_label: end_label.clone(),
                });
                self.line(&format!("{start_label}:"));
                if !condition_is_always_true && !self.emit_jump_if_false(condition, &end_label)? {
                    self.emit_expr_to_a(condition)?;
                    self.line("    or a");
                    self.line(&format!("    jp z, {end_label}"));
                }
                self.emit_block(body)?;
                self.line(&format!("    jp {start_label}"));
                self.line(&format!("{end_label}:"));
                self.loop_stack.pop();
            }
            Stmt::Loop { body } => {
                let start_label = self.next_label("loop");
                let end_label = self.next_label("endloop");
                self.loop_stack.push(LoopLabels {
                    continue_label: start_label.clone(),
                    break_label: end_label.clone(),
                });
                self.line(&format!("{start_label}:"));
                self.emit_block(body)?;
                self.line(&format!("    jp {start_label}"));
                self.line(&format!("{end_label}:"));
                self.loop_stack.pop();
            }
            Stmt::Break => {
                let Some(labels) = self.loop_stack.last() else {
                    return Err(Diagnostic::new("`break` outside loop"));
                };
                self.line(&format!("    jp {}", labels.break_label));
            }
            Stmt::Continue => {
                let Some(labels) = self.loop_stack.last() else {
                    return Err(Diagnostic::new("`continue` outside loop"));
                };
                self.line(&format!("    jp {}", labels.continue_label));
            }
            Stmt::Return(None) => {
                if self.current_function_returns_two_values() {
                    return Err(Diagnostic::new(format!(
                        "two-result function `{}` must return two values",
                        self.current_function_name()
                    )));
                }
                if self.current_function_requires_return_value() {
                    return Err(Diagnostic::new(format!(
                        "missing return value in function `{}`",
                        self.current_function_name()
                    )));
                }
                if self.current_function_uses_frame() {
                    self.emit_frame_epilogue();
                }
                if self.current_function_is_interrupt() {
                    self.emit_interrupt_epilogue();
                } else {
                    self.line("    ret");
                }
            }
            Stmt::Return(Some(expr)) => {
                if self.current_function_returns_two_values() {
                    if let Expr::Call { path, args } = expr {
                        let name = path_text(path);
                        if self.call_returns_two_values(&name, args)? {
                            self.emit_forward_two_result_call(&name, args)?;
                            return Ok(());
                        }
                    }
                    return Err(Diagnostic::new(format!(
                        "two-result function `{}` must use `return first, second`",
                        self.current_function_name()
                    )));
                }
                if let Expr::Call { path, args } = expr
                    && self.emit_approved_tail_call(&path_text(path), args)?
                {
                    return Ok(());
                }
                if !self.current_function_requires_return_value() {
                    return Err(Diagnostic::new(format!(
                        "void function `{}` cannot return a value",
                        self.current_function_name()
                    )));
                }
                let return_type = self.current_return_type().clone();
                self.emit_expr_to_type(expr, &return_type)?;
                if self.current_function_uses_frame() {
                    self.emit_frame_epilogue();
                }
                if self.current_function_is_interrupt() {
                    self.emit_interrupt_epilogue();
                } else {
                    self.line("    ret");
                }
            }
            Stmt::ReturnTwo { first, second } => {
                self.emit_return_two(first, second)?;
            }
            Stmt::Asm {
                volatile,
                inputs,
                outputs,
                clobbers,
                lines,
            } => self.emit_inline_asm(*volatile, inputs, outputs, clobbers, lines)?,
        }
        Ok(())
    }

    fn emit_inline_asm(
        &mut self,
        volatile: bool,
        inputs: &[crate::ast::AsmInput],
        outputs: &[crate::ast::AsmOutput],
        clobbers: &[String],
        lines: &[String],
    ) -> Result<(), Diagnostic> {
        let mut operands = HashMap::new();

        if volatile {
            self.line("    ; asm volatile");
        } else {
            self.line("    ; asm");
        }
        for input in inputs {
            if operands.contains_key(&input.name) {
                return Err(Diagnostic::new(format!(
                    "duplicate inline asm operand `{}`",
                    input.name
                )));
            }
            self.validate_inline_asm_input_type(input)?;
            let binding = self.inline_asm_input_binding(input)?;
            self.line(&format!(
                "    ; in {}: {} as {}",
                input.name,
                type_display(&input.ty),
                input.class
            ));
            operands.insert(input.name.clone(), binding);
        }
        for output in outputs {
            if operands.contains_key(&output.name) {
                return Err(Diagnostic::new(format!(
                    "duplicate inline asm operand `{}`",
                    output.name
                )));
            }
            self.validate_inline_asm_output_type(output)?;
            let binding = self.inline_asm_output_binding(output)?;
            self.line(&format!(
                "    ; out {}: {} as {}",
                output.name,
                type_display(&output.ty),
                output.class
            ));
            operands.insert(output.name.clone(), binding);
        }
        if !clobbers.is_empty() {
            self.line(&format!("    ; clobber {}", clobbers.join(", ")));
        }
        if (inputs.iter().any(|input| input.class == "mem")
            || outputs.iter().any(|output| output.class == "mem"))
            && !asm_clobbers_include(clobbers, "memory")
        {
            return Err(Diagnostic::new(
                "inline asm uses memory without declaring clobber `memory`",
            ));
        }
        let substituted_lines = lines
            .iter()
            .map(|line| substitute_inline_asm_operands(line, &operands))
            .collect::<Result<Vec<_>, _>>()?;
        validate_inline_asm_clobbers(
            clobbers,
            &substituted_lines,
            self.current_function_is_naked(),
            self.cpu.into(),
        )?;

        for input in inputs {
            self.emit_inline_asm_input_load(input)?;
        }
        let preserve_ix = !self.current_function_is_naked() && asm_clobbers_include(clobbers, "ix");
        let preserve_iy = !self.current_function_is_naked() && asm_clobbers_include(clobbers, "iy");
        if preserve_ix {
            self.line("    push ix");
        }
        if preserve_iy {
            self.line("    push iy");
        }
        for line in &substituted_lines {
            self.line(&format!("    {line}"));
        }
        if preserve_iy {
            self.line("    pop iy");
        }
        if preserve_ix {
            self.line("    pop ix");
        }
        for output in outputs {
            self.emit_inline_asm_output_store(output)?;
            self.invalidate_local_constant(&output.name);
            self.invalidate_readonly_pointer_alias(&output.name);
        }
        self.line("    ; end asm");
        if asm_clobbers_include(clobbers, "memory") {
            self.invalidate_all_local_constants();
        }
        Ok(())
    }

    fn validate_inline_asm_input_type(
        &self,
        input: &crate::ast::AsmInput,
    ) -> Result<(), Diagnostic> {
        let Some(bound) = self.named_value_type(&input.name) else {
            return Err(Diagnostic::new(format!("unknown value `{}`", input.name)));
        };
        let declared = self.symbols.resolved_type(&input.ty)?;
        let bound = self.symbols.resolved_type(bound)?;
        if declared != bound {
            return Err(Diagnostic::new(format!(
                "inline asm input `{}` declared type `{}` does not match bound type `{}`",
                input.name,
                type_display(&declared),
                type_display(&bound)
            )));
        }
        Ok(())
    }

    fn validate_inline_asm_output_type(
        &self,
        output: &crate::ast::AsmOutput,
    ) -> Result<(), Diagnostic> {
        let Some(bound) = self.variable_type(&output.name) else {
            return Err(Diagnostic::new(format!(
                "unknown variable `{}`",
                output.name
            )));
        };
        let declared = self.symbols.resolved_type(&output.ty)?;
        let bound = self.symbols.resolved_type(bound)?;
        if declared != bound {
            return Err(Diagnostic::new(format!(
                "inline asm output `{}` declared type `{}` does not match bound type `{}`",
                output.name,
                type_display(&declared),
                type_display(&bound)
            )));
        }
        Ok(())
    }

    fn inline_asm_input_binding(&self, input: &crate::ast::AsmInput) -> Result<String, Diagnostic> {
        match input.class.as_str() {
            "reg8" => Ok("a".to_owned()),
            "reg16" | "reg24" => Ok("hl".to_owned()),
            "mem" => {
                let variable = self.variable(&input.name)?;
                Ok(format!("({:06X}h)", variable.addr))
            }
            "imm" => {
                let width = self.symbols.type_width(&input.ty)?;
                let value = self.eval_i64_with_local_constants(&Expr::Ident(input.name.clone()))?;
                Ok(format_immediate(value, width))
            }
            _ => Err(Diagnostic::new(format!(
                "unsupported inline asm operand class `{}`",
                input.class
            ))),
        }
    }

    fn inline_asm_output_binding(
        &self,
        output: &crate::ast::AsmOutput,
    ) -> Result<String, Diagnostic> {
        match output.class.as_str() {
            "reg8" => Ok("a".to_owned()),
            "reg16" | "reg24" => Ok("hl".to_owned()),
            "mem" => {
                let variable = self.variable(&output.name)?;
                Ok(format!("({:06X}h)", variable.addr))
            }
            "imm" => Err(Diagnostic::new(format!(
                "inline asm output `{}` cannot use imm class",
                output.name
            ))),
            _ => Err(Diagnostic::new(format!(
                "unsupported inline asm operand class `{}`",
                output.class
            ))),
        }
    }

    fn emit_inline_asm_input_load(
        &mut self,
        input: &crate::ast::AsmInput,
    ) -> Result<(), Diagnostic> {
        match input.class.as_str() {
            "reg8" => {
                if let Some(variable) = self.variable_opt(&input.name) {
                    self.emit_load_a(variable);
                } else {
                    let value =
                        self.eval_i64_with_local_constants(&Expr::Ident(input.name.clone()))?;
                    self.line(&format!("    ld a, {value:02X}h"));
                }
            }
            "reg16" | "reg24" => {
                let width = self.symbols.type_width(&input.ty)?;
                if let Some(variable) = self.variable_opt(&input.name) {
                    self.emit_load_width(variable);
                } else {
                    let value =
                        self.eval_i64_with_local_constants(&Expr::Ident(input.name.clone()))?;
                    self.line(&format!("    ld hl, {}", format_immediate(value, width)));
                }
            }
            "mem" | "imm" => {}
            _ => {
                return Err(Diagnostic::new(format!(
                    "unsupported inline asm operand class `{}`",
                    input.class
                )));
            }
        }
        Ok(())
    }

    fn emit_inline_asm_output_store(
        &mut self,
        output: &crate::ast::AsmOutput,
    ) -> Result<(), Diagnostic> {
        match output.class.as_str() {
            "reg8" | "reg16" | "reg24" => {
                let variable = self.variable(&output.name)?;
                self.emit_store_width(variable);
            }
            "mem" => {}
            "imm" => {
                return Err(Diagnostic::new(format!(
                    "inline asm output `{}` cannot use imm class",
                    output.name
                )));
            }
            _ => {
                return Err(Diagnostic::new(format!(
                    "unsupported inline asm operand class `{}`",
                    output.class
                )));
            }
        }
        Ok(())
    }

    fn emit_assignment_value(
        &mut self,
        variable: Variable,
        op: AssignOp,
        value: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if variable.size == 2 {
            match op {
                AssignOp::Set => self.emit_expr_to_hl(value, variable.width()?)?,
                AssignOp::Add => self.emit_wide_assignment_op(variable, BinaryOp::Add, value)?,
                AssignOp::Sub => self.emit_wide_assignment_op(variable, BinaryOp::Sub, value)?,
                AssignOp::Mul => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Mul, value, signed)?
                }
                AssignOp::Div => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Div, value, signed)?
                }
                AssignOp::Mod => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Mod, value, signed)?
                }
                AssignOp::BitAnd => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitAnd, value)?
                }
                AssignOp::BitOr => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitOr, value)?
                }
                AssignOp::BitXor => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitXor, value)?
                }
                AssignOp::Shl => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shl, value, signed)?
                }
                AssignOp::Shr => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shr, value, signed)?
                }
            }
            return Ok(());
        }
        if variable.size == 3 {
            match op {
                AssignOp::Set => self.emit_expr_to_hl(value, ValueWidth::U24)?,
                AssignOp::Add => self.emit_wide_assignment_op(variable, BinaryOp::Add, value)?,
                AssignOp::Sub => self.emit_wide_assignment_op(variable, BinaryOp::Sub, value)?,
                AssignOp::Mul => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Mul, value, signed)?
                }
                AssignOp::Div => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Div, value, signed)?
                }
                AssignOp::Mod => {
                    self.emit_arithmetic_assignment_op(variable, BinaryOp::Mod, value, signed)?
                }
                AssignOp::BitAnd => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitAnd, value)?
                }
                AssignOp::BitOr => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitOr, value)?
                }
                AssignOp::BitXor => {
                    self.emit_wide_assignment_op(variable, BinaryOp::BitXor, value)?
                }
                AssignOp::Shl => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shl, value, signed)?
                }
                AssignOp::Shr => {
                    self.emit_wide_assignment_shift(variable, BinaryOp::Shr, value, signed)?
                }
            }
            return Ok(());
        }

        match op {
            AssignOp::Set => self.emit_expr_to_a(value)?,
            AssignOp::Add => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    add a, b");
            }
            AssignOp::Sub => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    ld c, a");
                self.line("    ld a, b");
                self.line("    sub c");
            }
            AssignOp::Mul => {
                self.emit_arithmetic_assignment_op(variable, BinaryOp::Mul, value, signed)?
            }
            AssignOp::Div => {
                self.emit_arithmetic_assignment_op(variable, BinaryOp::Div, value, signed)?
            }
            AssignOp::Mod => {
                self.emit_arithmetic_assignment_op(variable, BinaryOp::Mod, value, signed)?
            }
            AssignOp::BitAnd => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    and b");
            }
            AssignOp::BitOr => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    or b");
            }
            AssignOp::BitXor => {
                self.emit_load_a(variable);
                self.line("    ld b, a");
                self.emit_expr_to_a(value)?;
                self.line("    xor b");
            }
            AssignOp::Shl => {
                self.ensure_shift_count_compatible(value)?;
                self.emit_load_a(variable);
                self.emit_shift_a_by_expr(BinaryOp::Shl, value, signed)?;
            }
            AssignOp::Shr => {
                self.ensure_shift_count_compatible(value)?;
                self.emit_load_a(variable);
                self.emit_shift_a_by_expr(BinaryOp::Shr, value, signed)?;
            }
        }
        Ok(())
    }

    fn emit_local_single_bit_compound_assignment(
        &mut self,
        variable: Variable,
        ty: &Type,
        op: AssignOp,
        value: &Expr,
    ) -> Result<bool, Diagnostic> {
        if !supports_z80_bit_instructions(self.cpu) {
            return Ok(false);
        }
        let ty = self.symbols.resolved_type(ty)?;
        if !matches!(ty, Type::Named(ref name) if matches!(name.as_str(), "u8" | "u16" | "u24")) {
            return Ok(false);
        }
        let width_mask = (1_i64 << (variable.size * 8)) - 1;
        let Ok(mask) = self.eval_i64_with_local_constants(value) else {
            return Ok(false);
        };
        let mask = mask & width_mask;
        let changed_bit = match op {
            AssignOp::BitOr if mask.count_ones() == 1 => mask,
            AssignOp::BitAnd if (!mask & width_mask).count_ones() == 1 => !mask & width_mask,
            _ => return Ok(false),
        };
        let bit = changed_bit.trailing_zeros();
        let address = variable.addr + bit / 8;
        self.line(&format!("    ld hl, {address:06X}h"));
        match op {
            AssignOp::BitOr => self.line(&format!("    set {}, (hl)", bit % 8)),
            AssignOp::BitAnd => self.line(&format!("    res {}, (hl)", bit % 8)),
            _ => unreachable!("single-bit compound assignment only uses AND/OR"),
        }
        Ok(true)
    }

    fn emit_arithmetic_assignment_op(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        value: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        let width = variable.width()?;
        match op {
            BinaryOp::Mul => self.emit_assignment_mul(variable, value, width, signed),
            BinaryOp::Div | BinaryOp::Mod => {
                if signed {
                    self.emit_signed_assignment_div_mod(variable, value, op, width)
                } else {
                    self.emit_unsigned_assignment_div_mod(variable, value, op, width)
                }
            }
            _ => unreachable!("not an arithmetic compound assignment op"),
        }
    }

    fn emit_assignment_mul(
        &mut self,
        variable: Variable,
        value: &Expr,
        width: ValueWidth,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if let Some(factor) = self.constant_integer_value(value)?
            && let Some((raw, form)) = self.constant_mul_choice(width, factor)
        {
            self.emit_load_width(variable);
            self.emit_constant_mul_from_loaded(width, raw, signed, &form);
            return Ok(());
        }
        if width == ValueWidth::U8 {
            let left = self.alloc_var(width.bytes());
            self.emit_load_a(variable);
            self.emit_store_a(left);
            self.emit_expr_to_a(value)?;
            self.line("    ld c, a");
            self.emit_load_a(left);
            self.line("    call __ezra_mul_u8");
            return Ok(());
        }

        self.emit_load_width(variable);
        self.line("    push hl");
        self.emit_expr_to_hl(value, width)?;
        self.line("    push hl");
        self.line("    pop bc");
        self.line("    pop hl");
        match width {
            ValueWidth::U16 => self.line("    call __ezra_mul_u16"),
            ValueWidth::U24 if signed => self.line("    call __ezra_mul_i24"),
            ValueWidth::U24 => self.line("    call __ezra_mul_u24"),
            ValueWidth::U8 => unreachable!("u8 handled above"),
        }
        Ok(())
    }

    fn emit_unsigned_assignment_div_mod(
        &mut self,
        variable: Variable,
        value: &Expr,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U8 {
            let left = self.alloc_var(width.bytes());
            self.emit_load_a(variable);
            self.emit_store_a(left);
            self.emit_expr_to_a(value)?;
            self.line("    ld c, a");
            self.emit_load_a(left);
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_u8"),
                BinaryOp::Mod => self.line("    call __ezra_mod_u8"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }

        self.emit_load_width(variable);
        self.line("    push hl");
        self.emit_expr_to_hl(value, width)?;
        self.line("    push hl");
        self.line("    pop bc");
        self.line("    pop hl");
        match (op, width) {
            (BinaryOp::Div, ValueWidth::U16) => self.line("    call __ezra_div_u16"),
            (BinaryOp::Mod, ValueWidth::U16) => self.line("    call __ezra_mod_u16"),
            (BinaryOp::Div, ValueWidth::U24) => self.line("    call __ezra_div_u24"),
            (BinaryOp::Mod, ValueWidth::U24) => self.line("    call __ezra_mod_u24"),
            _ => unreachable!("unsupported unsigned assignment division width"),
        }
        Ok(())
    }

    fn emit_signed_assignment_div_mod(
        &mut self,
        variable: Variable,
        value: &Expr,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U24 {
            self.emit_load_width(variable);
            self.line("    push hl");
            self.emit_expr_to_hl(value, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_i24"),
                BinaryOp::Mod => self.line("    call __ezra_mod_i24"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }

        let dividend = self.alloc_var(width.bytes());
        let divisor = self.alloc_var(width.bytes());
        let quotient = self.alloc_var(width.bytes());
        let quotient_negative = self.alloc_var(ValueWidth::U8.bytes());
        let remainder_negative = self.alloc_var(ValueWidth::U8.bytes());
        let loop_label = self.next_label("sdiv_loop");
        let zero_label = self.next_label("sdiv_zero");
        let done_label = self.next_label("sdiv_done");
        let quotient_positive_label = self.next_label("sdiv_q_positive");
        let remainder_positive_label = self.next_label("sdiv_r_positive");
        let not_overflow_label = self.next_label("sdiv_not_overflow");
        let finished_label = self.next_label("sdiv_finished");

        self.emit_load_width(variable);
        self.emit_store_width(dividend);
        self.emit_expr_to_width(value, width)?;
        self.emit_store_width(divisor);
        self.emit_zero_variable(quotient);
        self.emit_zero_variable(quotient_negative);
        self.emit_zero_variable(remainder_negative);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_jump_if_memory_not_equals(dividend, signed_min_bytes(width), &not_overflow_label);
        self.emit_jump_if_memory_not_equals(
            divisor,
            signed_negative_one_bytes(width),
            &not_overflow_label,
        );
        match op {
            BinaryOp::Div => self.emit_load_width(dividend),
            BinaryOp::Mod => {
                self.emit_zero_variable(dividend);
                self.emit_load_width(dividend);
            }
            _ => unreachable!("not a division op"),
        }
        self.line(&format!("    jp {finished_label}"));
        self.line(&format!("{not_overflow_label}:"));

        self.emit_abs_signed_variable(dividend, Some(quotient_negative), Some(remainder_negative));
        self.emit_abs_signed_variable(divisor, Some(quotient_negative), None);

        self.line(&format!("{loop_label}:"));
        if width == ValueWidth::U8 {
            self.emit_load_a(dividend);
            self.line("    ld b, a");
            self.emit_load_a(divisor);
            self.line("    ld c, a");
            self.line("    ld a, b");
            self.line("    cp c");
            self.line(&format!("    jp c, {done_label}"));
            self.line("    sub c");
            self.emit_store_a(dividend);
        } else {
            self.emit_load_width(dividend);
            self.line("    push hl");
            self.emit_load_width(divisor);
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, de");
            self.line(&format!("    jp c, {done_label}"));
            self.emit_store_width(dividend);
        }
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        self.emit_load_a(quotient_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {quotient_positive_label}"));
        self.emit_negate_memory(quotient);
        self.line(&format!("{quotient_positive_label}:"));
        self.emit_load_a(remainder_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {remainder_positive_label}"));
        self.emit_negate_memory(dividend);
        self.line(&format!("{remainder_positive_label}:"));

        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        self.line(&format!("{finished_label}:"));
        Ok(())
    }

    fn emit_typed_assignment_value(
        &mut self,
        variable: Variable,
        ty: &Type,
        op: AssignOp,
        value: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if op != AssignOp::Set {
            let resolved = self.symbols.resolved_type(ty)?;
            match &resolved {
                Type::Ptr(pointee) => {
                    return self.emit_pointer_compound_assignment(variable, pointee, op, value);
                }
                Type::Array { .. } => return Err(Diagnostic::new("type mismatch")),
                Type::Named(name) if name == "bool" || self.symbols.structs.contains_key(name) => {
                    return Err(Diagnostic::new("type mismatch"));
                }
                _ => {}
            }
        }
        self.emit_assignment_value(variable, op, value, signed)
    }

    fn emit_pointer_compound_assignment(
        &mut self,
        variable: Variable,
        pointee: &Type,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let binary_op = match op {
            AssignOp::Add => BinaryOp::Add,
            AssignOp::Sub => BinaryOp::Sub,
            _ => return Err(Diagnostic::new("type mismatch")),
        };
        self.ensure_pointer_offset_expr(value)?;
        let scale = self.symbols.type_size(pointee)?;
        if scale == 1 && is_unit_integer_literal(value) {
            self.emit_load_width(variable);
            match binary_op {
                BinaryOp::Add => self.line("    inc hl"),
                BinaryOp::Sub => self.line("    dec hl"),
                _ => unreachable!("pointer compound assignment only uses add/sub"),
            }
            return Ok(());
        }
        self.emit_load_width(variable);
        self.line("    push hl");
        self.emit_scaled_offset_to_hl(value, scale)?;
        match binary_op {
            BinaryOp::Add => {
                self.line("    pop bc");
                self.line("    add hl, bc");
            }
            BinaryOp::Sub => {
                self.line("    ex de, hl");
                self.line("    pop hl");
                self.line("    or a");
                self.line("    sbc hl, de");
            }
            _ => unreachable!("pointer compound assignment only uses add/sub"),
        }
        Ok(())
    }

    fn ensure_compound_assignment_target(&self, ty: &Type, op: AssignOp) -> Result<(), Diagnostic> {
        if op == AssignOp::Set {
            return Ok(());
        }
        match self.symbols.resolved_type(ty)? {
            Type::Ptr(_) if matches!(op, AssignOp::Add | AssignOp::Sub) => Ok(()),
            Type::Ptr(_) | Type::Function { .. } => Err(Diagnostic::new("type mismatch")),
            Type::Array { .. } => Err(Diagnostic::new("type mismatch")),
            Type::Named(name) if name == "bool" || self.symbols.structs.contains_key(&name) => {
                Err(Diagnostic::new("type mismatch"))
            }
            Type::Named(_) => Ok(()),
        }
    }

    fn emit_assignment(
        &mut self,
        target: &Place,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        match target {
            Place::Ident(name) => {
                self.invalidate_local_constant(name);
                self.invalidate_readonly_pointer_alias(name);
                let variable = self.variable(name)?;
                let ty = self.variable_type(name).cloned();
                if op == AssignOp::Set
                    && let Some(ty) = ty.as_ref()
                {
                    if is_self_unit_pointer_increment(name, value)
                        && matches!(self.symbols.resolved_type(ty)?, Type::Ptr(ref pointee) if self.symbols.type_size(pointee)? == 1)
                    {
                        self.emit_load_width(variable);
                        self.line("    inc hl");
                        self.emit_store_width(variable);
                    } else {
                        self.emit_storage_initializer(variable, ty, value)?;
                        self.record_local_constant(name, ty, value);
                        self.record_readonly_pointer_alias(name, value);
                    }
                    return Ok(());
                }
                if let (Some(local), Some(local_ty)) = (
                    self.scopes
                        .iter()
                        .rev()
                        .find_map(|scope| scope.get(name))
                        .copied(),
                    self.scope_types
                        .iter()
                        .rev()
                        .find_map(|scope| scope.get(name))
                        .cloned(),
                ) && self
                    .emit_local_single_bit_compound_assignment(local, &local_ty, op, value)?
                {
                    return Ok(());
                }
                let signed = self
                    .variable_type(name)
                    .map(|ty| self.type_is_signed(ty))
                    .transpose()?
                    .unwrap_or(false);
                if let Some(ty) = ty.as_ref() {
                    self.emit_typed_assignment_value(variable, ty, op, value, signed)?;
                } else {
                    self.emit_assignment_value(variable, op, value, signed)?;
                }
                self.emit_store_width(variable);
            }
            Place::Index { name, index } => {
                self.emit_index_assignment(name, index, op, value)?;
            }
            Place::Field { base, field } => {
                let variable = self.field_variable(base, field)?;
                if op == AssignOp::Set {
                    let ty = self.field_type(base, field)?;
                    self.validate_expr_assignable_to_type(value, &ty)?;
                    self.emit_storage_initializer(variable, &ty, value)?;
                    return Ok(());
                }
                let ty = self.field_type(base, field)?;
                self.ensure_compound_assignment_target(&ty, op)?;
                variable.width()?;
                let signed = self.type_is_signed(&ty)?;
                self.emit_typed_assignment_value(variable, &ty, op, value, signed)?;
                self.emit_store_width(variable);
            }
            Place::Access(path) => {
                self.emit_access_assignment(path, op, value)?;
            }
            Place::Deref(ptr) => {
                self.emit_deref_assignment(ptr, op, value)?;
            }
        }
        Ok(())
    }

    fn emit_array_initializer(
        &mut self,
        variable: Variable,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let Expr::Array(values) = value else {
            return Err(Diagnostic::new(
                "array initializer must be an array literal",
            ));
        };
        let element_size = variable
            .element_size
            .ok_or_else(|| Diagnostic::new("scalar variable cannot use array initializer"))?;
        let len = variable
            .len
            .ok_or_else(|| Diagnostic::new("array variable missing length"))?;
        let Type::Array {
            element: element_ty,
            ..
        } = self.symbols.resolved_type(ty)?
        else {
            return Err(Diagnostic::new("array initializer requires an array type"));
        };
        if values.len() as u32 > len {
            return Err(Diagnostic::new(format!(
                "array initializer has {} values but array length is {len}",
                values.len()
            )));
        }
        for index in 0..len {
            let element_addr = variable.addr + index * element_size;
            let element = self.symbols.storage_at(element_addr, &element_ty)?;
            if let Some(value) = values.get(index as usize) {
                self.validate_expr_assignable_to_type(value, &element_ty)?;
                match self.symbols.resolved_type(&element_ty)? {
                    Type::Array { .. } => {
                        self.emit_array_initializer(element, &element_ty, value)?
                    }
                    Type::Named(name) if self.symbols.structs.contains_key(&name) => {
                        self.emit_struct_initializer(element, &element_ty, value)?
                    }
                    _ => {
                        self.emit_expr_to_width(value, element.width()?)?;
                        self.emit_store_width(element);
                    }
                }
            } else {
                self.emit_zero_storage(element);
            }
        }
        Ok(())
    }

    fn emit_struct_initializer(
        &mut self,
        variable: Variable,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let struct_name = self.struct_type_name(ty)?;
        let Expr::StructInit { ty, fields } = value else {
            return Err(Diagnostic::new(format!(
                "struct `{struct_name}` initializer must use `{struct_name} {{ ... }}`"
            )));
        };
        if ty != &struct_name {
            return Err(Diagnostic::new(format!(
                "initializer type `{ty}` does not match `{struct_name}`"
            )));
        }

        let layout = self
            .symbols
            .structs
            .get(&struct_name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown struct `{struct_name}`")))?;
        let mut initialized = HashMap::new();
        for (field_name, field_value) in fields {
            let Some(field) = layout.fields.get(field_name) else {
                return Err(Diagnostic::new(format!(
                    "struct `{struct_name}` has no field `{field_name}`"
                )));
            };
            if initialized.insert(field_name.clone(), ()).is_some() {
                return Err(Diagnostic::new(format!(
                    "duplicate initializer for field `{field_name}`"
                )));
            }
            let field_var = self
                .symbols
                .storage_at(variable.addr + field.offset, &field.ty)?;
            self.validate_expr_assignable_to_type(field_value, &field.ty)?;
            self.emit_storage_initializer(field_var, &field.ty, field_value)?;
        }

        for (field_name, field) in &layout.fields {
            if initialized.contains_key(field_name) {
                continue;
            }
            let field_var = self
                .symbols
                .storage_at(variable.addr + field.offset, &field.ty)?;
            self.emit_zero_storage(field_var);
        }
        Ok(())
    }

    fn emit_storage_initializer(
        &mut self,
        variable: Variable,
        ty: &Type,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        self.validate_expr_arithmetic_compatibility(value)?;
        self.validate_expr_assignable_to_type(value, ty)?;
        if let Expr::Deref(ptr) = value {
            return self.emit_copy_pointed_storage_into(ptr, variable);
        }
        if let Some(source) = self.expr_storage_variable(value)? {
            return self.emit_copy_storage_into(source, variable);
        }
        match self.symbols.resolved_type(ty)? {
            Type::Array { .. } => self.emit_array_initializer(variable, ty, value),
            Type::Named(name) if self.symbols.structs.contains_key(&name) => {
                self.emit_struct_initializer(variable, ty, value)
            }
            _ => {
                let width = variable.width()?;
                if width == ValueWidth::U24
                    && let Ok(value) = self.eval_i64_with_local_constants(value)
                {
                    self.validate_value_width_for_target((value as u32) & 0xFF_FFFF, width)?;
                }
                self.emit_expr_to_width(value, width)?;
                self.emit_store_width(variable);
                Ok(())
            }
        }
    }

    fn emit_wide_assignment_op(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        self.emit_load_width(variable);
        self.line("    push hl");
        self.emit_expr_to_hl(value, variable.width()?)?;
        self.line("    pop bc");
        self.emit_wide_op_with_left_in_bc(op, variable.width()?)?;
        Ok(())
    }

    fn emit_wide_assignment_shift(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        value: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        self.ensure_shift_count_compatible(value)?;
        let width = variable.width()?;
        if let Some(count) = self.maybe_const_shift_count(value)?
            && cacheable_ranges_contain(&self.cacheable_ranges, variable.addr, variable.size)
        {
            if count == 0 {
                self.emit_load_width(variable);
                return Ok(());
            }
            if count % 8 == 0 {
                self.emit_byte_aligned_shift_temporary(variable, op, count / 8, signed);
                self.emit_load_width(variable);
                return Ok(());
            }
        }

        let temp = self.alloc_var(width.bytes());
        self.emit_load_width(variable);
        self.emit_store_width(temp);
        self.emit_shift_temporary_by_expr(temp, op, value, signed)?;
        self.emit_load_width(temp);
        Ok(())
    }

    fn emit_call(&mut self, path: &[String], args: &[Expr]) -> Result<(), Diagnostic> {
        let name = path_text(path);
        if CATALOG.lookup(&name).is_some() {
            return self.emit_intrinsic_call(&name, args);
        }
        match name.as_str() {
            "test.pass" | "ezra.test.pass" => {
                self.line("    call __ezra_pass");
            }
            "test.fail" | "ezra.test.fail" => {
                let expr = args.first().cloned().unwrap_or(Expr::Int(1));
                self.validate_expr_assignable_to_type(&expr, &Type::Named("u8".to_owned()))?;
                self.emit_expr_to_a(&expr)?;
                self.emit_test_fail_call();
            }
            "test.assert_eq_u8" | "ezra.test.assert_eq_u8" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u8 requires three arguments",
                    ));
                }
                self.validate_expr_has_test_width(&args[0], ValueWidth::U8, true)?;
                self.validate_expr_has_test_width(&args[1], ValueWidth::U8, true)?;
                self.validate_expr_assignable_to_type(&args[2], &Type::Named("u8".to_owned()))?;
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_a(&args[0])?;
                self.line("    ld b, a");
                self.emit_expr_to_a(&args[1])?;
                self.line("    ld c, a");
                self.line("    ld a, b");
                self.line("    cp c");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_test_fail_call();
                self.line(&format!("{ok}:"));
            }
            "test.assert_eq_u16" | "ezra.test.assert_eq_u16" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u16 requires three arguments",
                    ));
                }
                self.validate_expr_has_test_width(&args[0], ValueWidth::U16, false)?;
                self.validate_expr_has_test_width(&args[1], ValueWidth::U16, false)?;
                self.validate_expr_assignable_to_type(&args[2], &Type::Named("u8".to_owned()))?;
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_hl(&args[0], ValueWidth::U16)?;
                self.line("    push hl");
                self.emit_expr_to_hl(&args[1], ValueWidth::U16)?;
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_test_fail_call();
                self.line(&format!("{ok}:"));
            }
            "test.assert_eq_u24" | "ezra.test.assert_eq_u24" => {
                if args.len() != 3 {
                    return Err(Diagnostic::new(
                        "test.assert_eq_u24 requires three arguments",
                    ));
                }
                self.validate_expr_has_test_width(&args[0], ValueWidth::U24, false)?;
                self.validate_expr_has_test_width(&args[1], ValueWidth::U24, false)?;
                self.validate_expr_assignable_to_type(&args[2], &Type::Named("u8".to_owned()))?;
                let ok = self.next_label("assert_ok");
                self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
                self.line("    push hl");
                self.emit_expr_to_hl(&args[1], ValueWidth::U24)?;
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {ok}"));
                self.emit_expr_to_a(&args[2])?;
                self.emit_test_fail_call();
                self.line(&format!("{ok}:"));
            }
            "debug.char" | "ezra.debug.char" => {
                let expr = args
                    .first()
                    .ok_or_else(|| Diagnostic::new("debug.char requires one argument"))?;
                self.validate_expr_assignable_to_type(expr, &Type::Named("u8".to_owned()))?;
                self.emit_expr_to_a(expr)?;
                self.emit_out_a(0x0C);
            }
            "debug.str" | "ezra.debug.str" => {
                self.emit_debug_str(args)?;
            }
            "debug.hex_u8" | "ezra.debug.hex_u8" => {
                self.emit_debug_hex(args, ValueWidth::U8)?;
            }
            "debug.hex_u16" | "ezra.debug.hex_u16" => {
                self.emit_debug_hex(args, ValueWidth::U16)?;
            }
            "debug.hex_u24" | "ezra.debug.hex_u24" => {
                self.emit_debug_hex(args, ValueWidth::U24)?;
            }
            "mem.poke8" | "ezra.mem.poke8" => {
                self.emit_mem_poke8(args)?;
            }
            "mem.memcpy" | "ezra.mem.memcpy" => {
                self.emit_memcpy(args)?;
            }
            "mem.memset" | "ezra.mem.memset" => {
                self.emit_memset(args)?;
            }
            path => self.emit_callable_call(path, args)?,
        }
        Ok(())
    }

    fn emit_callable_call(&mut self, name: &str, args: &[Expr]) -> Result<(), Diagnostic> {
        if self.symbols.functions.contains_key(name) {
            self.emit_user_call(name, args)
        } else if name.split_once('.').is_none() && self.variable_type(name).is_some() {
            self.emit_indirect_call(name, args)
        } else {
            self.emit_user_call(name, args)
        }
    }

    fn emit_test_fail_call(&mut self) {
        self.line("    call __ezra_fail");
    }

    fn emit_debug_str(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            return Err(Diagnostic::new("debug.str requires one argument"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;

        let cursor = self.alloc_var(ValueWidth::U24.bytes());
        let loop_label = self.next_label("debug_str");
        let done_label = self.next_label("debug_str_done");
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(cursor);
        self.line(&format!("{loop_label}:"));
        self.emit_load_hl(cursor);
        self.line("    ld a, (hl)");
        self.line("    or a");
        self.line(&format!("    jp z, {done_label}"));
        self.emit_out_a(0x0C);
        self.emit_load_hl(cursor);
        self.line("    inc hl");
        self.emit_store_hl(cursor);
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done_label}:"));
        Ok(())
    }

    fn emit_debug_hex(&mut self, args: &[Expr], width: ValueWidth) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            let suffix = match width {
                ValueWidth::U8 => "u8",
                ValueWidth::U16 => "u16",
                ValueWidth::U24 => "u24",
            };
            return Err(Diagnostic::new(format!(
                "debug.hex_{suffix} requires one argument"
            )));
        }
        if matches!(
            self.symbols.resolved_type(&self.expr_type(&args[0])?)?,
            Type::Array { .. }
        ) {
            return Err(Diagnostic::new("array value cannot be used as a scalar"));
        }
        self.validate_expr_assignable_to_type(&args[0], &width_unsigned_type(width))?;

        match width {
            ValueWidth::U8 => {
                self.emit_expr_to_a(&args[0])?;
                self.emit_debug_hex_byte_from_a();
            }
            ValueWidth::U16 | ValueWidth::U24 => {
                let value = self.alloc_var(width.bytes());
                self.emit_expr_to_hl(&args[0], width)?;
                self.emit_store_width(value);
                for offset in (0..width.bytes()).rev() {
                    self.line(&format!("    ld a, ({:06X}h)", value.addr + offset as u32));
                    self.emit_debug_hex_byte_from_a();
                }
            }
        }
        Ok(())
    }

    fn emit_debug_hex_byte_from_a(&mut self) {
        let byte = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_store_a(byte);
        self.emit_load_a(byte);
        for _ in 0..4 {
            self.line("    srl a");
        }
        self.emit_debug_hex_nibble_from_a();
        self.emit_load_a(byte);
        self.line("    ld bc, 00000Fh");
        self.line("    and c");
        self.emit_debug_hex_nibble_from_a();
    }

    fn emit_debug_hex_nibble_from_a(&mut self) {
        let digit_label = self.next_label("debug_hex_digit");
        let end_label = self.next_label("debug_hex_end");
        self.line("    ld bc, 00000Ah");
        self.line("    cp c");
        self.line(&format!("    jp c, {digit_label}"));
        self.line("    ld bc, 000037h");
        self.line("    add a, c");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{digit_label}:"));
        self.line("    ld bc, 000030h");
        self.line("    add a, c");
        self.line(&format!("{end_label}:"));
        self.emit_out_a(0x0C);
    }

    fn emit_approved_tail_call(&mut self, name: &str, args: &[Expr]) -> Result<bool, Diagnostic> {
        let edge = (self.current_function_name().to_owned(), name.to_owned());
        if !self.tail_call_edges.contains(&edge) {
            return Ok(false);
        }
        let sig = self
            .symbols
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if sig.arity != args.len()
            || sig.uses_arg_slots
            || sig.stack_arg_bytes != 0
            || sig.second_return_type.is_some()
            || self.current_function_returns_two_values()
        {
            return Ok(false);
        }

        let mut temps = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let width = sig.params[index];
            let ty = &sig.param_types[index];
            let temp = self.alloc_var(width.bytes());
            self.emit_expr_to_type(arg, ty)?;
            self.emit_store_width(temp);
            temps.push(temp);
        }
        if let Some(temp) = temps.get(2).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld c, a");
            } else if sig.params.get(1).is_some_and(|width| width.bytes() != 1) {
                self.emit_load_width(temp);
                self.line("    push hl");
                self.line("    pop bc");
            } else {
                return Ok(false);
            }
        }
        if let Some(temp) = temps.get(1).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld b, a");
            } else {
                self.emit_load_width(temp);
                self.line("    ex de, hl");
            }
        }
        if let Some(temp) = temps.first().copied() {
            self.emit_load_width(temp);
        }
        self.line(&format!("    jp {}", function_label(name)));
        Ok(true)
    }

    fn validate_two_result_value(
        &self,
        value: &Expr,
        first_ty: &Type,
        second_ty: &Type,
    ) -> Result<(), Diagnostic> {
        let Expr::Call { path, args } = value else {
            return Err(Diagnostic::new(
                "two-result bindings require a direct two-result call",
            ));
        };
        let name = path_text(path);
        if CATALOG.lookup(&name).is_some() {
            let resolution = self.resolve_intrinsic_call(&name, args)?;
            if resolution.result_count() != ResultCount::Two {
                return Err(Diagnostic::new(format!(
                    "intrinsic `{name}` does not return two values"
                )));
            }
            self.validate_type_assignable_to_type(&resolution.result_types[0], first_ty)?;
            self.validate_type_assignable_to_type(&resolution.result_types[1], second_ty)?;
            return Ok(());
        }
        let sig = self
            .symbols
            .functions
            .get(&name)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if sig.second_return_type.is_none() {
            return Err(Diagnostic::new(format!(
                "function `{name}` does not return two values"
            )));
        }
        if sig.arity != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments but got {}",
                sig.arity,
                args.len()
            )));
        }
        self.validate_type_assignable_to_type(
            sig.return_type
                .as_ref()
                .expect("two-result signature has a first return type"),
            first_ty,
        )?;
        self.validate_type_assignable_to_type(
            sig.second_return_type
                .as_ref()
                .expect("two-result signature has a second return type"),
            second_ty,
        )?;
        Ok(())
    }

    fn emit_two_result_value(
        &mut self,
        value: &Expr,
        second_destination: Variable,
    ) -> Result<(), Diagnostic> {
        let Expr::Call { path, args } = value else {
            return Err(Diagnostic::new(
                "two-result bindings require a direct two-result call",
            ));
        };
        self.emit_two_result_call(&path_text(path), args, second_destination)
    }

    fn emit_two_result_intrinsic_call(
        &mut self,
        name: &str,
        args: &[Expr],
        second_destination: Variable,
    ) -> Result<(), Diagnostic> {
        let resolution = self.resolve_intrinsic_call(name, args)?;
        if resolution.result_count() != ResultCount::Two {
            return Err(Diagnostic::new(format!(
                "intrinsic `{}` does not return two values",
                resolution.canonical_name()
            )));
        }
        match resolution.descriptor.operation {
            IntrinsicOperation::Int(IntIntrinsic::Divmod) => {
                self.emit_divmod_intrinsic(name, args, second_destination)
            }
            IntrinsicOperation::Int(IntIntrinsic::AddCarry) => {
                self.emit_carry_intrinsic(name, args, second_destination, false)
            }
            IntrinsicOperation::Int(IntIntrinsic::SubBorrow) => {
                self.emit_carry_intrinsic(name, args, second_destination, true)
            }
            IntrinsicOperation::Int(IntIntrinsic::FullMul) => {
                self.emit_full_mul_intrinsic(name, args, second_destination)
            }
            IntrinsicOperation::Mem(MemIntrinsic::FindByte) => {
                self.emit_find_byte_intrinsic(name, args, second_destination)
            }
            _ => Err(Diagnostic::new(format!(
                "intrinsic `{}` has an unsupported two-result operation",
                resolution.canonical_name()
            ))),
        }
    }

    fn emit_divmod_intrinsic(
        &mut self,
        name: &str,
        args: &[Expr],
        second_destination: Variable,
    ) -> Result<(), Diagnostic> {
        let types = self.intrinsic_argument_types(name, args)?;
        let ty = self.symbols.resolved_type(&types[0])?;
        let signed = type_is_signed(&ty);
        let temps = self.intrinsic_argument_temps(name, args)?;
        let dividend = self.alloc_var(temps[0].size);
        let divisor = self.alloc_var(temps[1].size);
        let quotient = self.alloc_var(temps[0].size);
        let remainder = self.alloc_var(temps[0].size);
        self.emit_copy_bytes(temps[0], 0, dividend, 0, dividend.size);
        self.emit_copy_bytes(temps[1], 0, divisor, 0, divisor.size);
        self.emit_divmod_variables(dividend, divisor, quotient, remainder, signed);
        self.emit_load_width(remainder);
        self.emit_store_width(second_destination);
        self.emit_load_width(quotient);
        Ok(())
    }

    fn emit_carry_intrinsic(
        &mut self,
        name: &str,
        args: &[Expr],
        second_destination: Variable,
        subtract: bool,
    ) -> Result<(), Diagnostic> {
        let temps = self.intrinsic_argument_temps(name, args)?;
        let result = self.alloc_var(temps[0].size);
        self.emit_carry_memory_op(
            result,
            temps[0],
            temps[1],
            temps[2],
            second_destination,
            subtract,
        );
        self.emit_load_width(result);
        Ok(())
    }

    fn emit_carry_memory_op(
        &mut self,
        result: Variable,
        left: Variable,
        right: Variable,
        carry_in: Variable,
        carry_out: Variable,
        subtract: bool,
    ) {
        let clear_carry = self.next_label("intrinsic_carry_clear");
        let start = self.next_label("intrinsic_carry_start");
        let carry_true = self.next_label("intrinsic_carry_true");
        let done = self.next_label("intrinsic_carry_done");
        self.emit_load_a(carry_in);
        self.line("    or a");
        self.line(&format!("    jp z, {clear_carry}"));
        self.line("    scf");
        self.line(&format!("    jp {start}"));
        self.line(&format!("{clear_carry}:"));
        self.line("    or a");
        self.line(&format!("{start}:"));
        for offset in 0..result.size {
            if subtract {
                self.line(&format!("    ld a, ({:06X}h)", right.addr + offset));
                self.line("    ld b, a");
                self.line(&format!("    ld a, ({:06X}h)", left.addr + offset));
            } else {
                self.line(&format!("    ld a, ({:06X}h)", left.addr + offset));
                self.line("    ld b, a");
                self.line(&format!("    ld a, ({:06X}h)", right.addr + offset));
            }
            self.line(if subtract {
                "    sbc a, b"
            } else {
                "    adc a, b"
            });
            self.line(&format!("    ld ({:06X}h), a", result.addr + offset));
        }
        self.line(&format!("    jp c, {carry_true}"));
        self.line("    xor a");
        self.line(&format!("    ld ({:06X}h), a", carry_out.addr));
        self.line(&format!("    jp {done}"));
        self.line(&format!("{carry_true}:"));
        self.line("    ld a, 01h");
        self.line(&format!("    ld ({:06X}h), a", carry_out.addr));
        self.line(&format!("{done}:"));
    }

    fn emit_full_mul_intrinsic(
        &mut self,
        name: &str,
        args: &[Expr],
        second_destination: Variable,
    ) -> Result<(), Diagnostic> {
        let types = self.intrinsic_argument_types(name, args)?;
        let ty = self.symbols.resolved_type(&types[0])?;
        let signed = type_is_signed(&ty);
        let temps = self.intrinsic_argument_temps(name, args)?;
        let product = self.alloc_var(temps[0].size * 2);
        self.emit_full_product(temps[0], temps[1], product, signed);
        let low = self.alloc_var(temps[0].size);
        let high = self.alloc_var(temps[0].size);
        self.emit_copy_bytes(product, 0, low, 0, low.size);
        self.emit_copy_bytes(product, high.size, high, 0, high.size);
        self.emit_load_width(high);
        self.emit_store_width(second_destination);
        self.emit_load_width(low);
        Ok(())
    }

    fn emit_find_byte_intrinsic(
        &mut self,
        name: &str,
        args: &[Expr],
        second_destination: Variable,
    ) -> Result<(), Diagnostic> {
        let temps = self.intrinsic_argument_temps(name, args)?;
        let found = self.next_label("intrinsic_find_found");
        let not_found = self.next_label("intrinsic_find_not_found");
        let done = self.next_label("intrinsic_find_done");
        let loop_label = self.next_label("intrinsic_find_loop");
        self.line(&format!("{loop_label}:"));
        self.emit_jump_if_memory_zero(temps[1], &not_found);
        self.emit_load_a(temps[2]);
        self.line("    ld c, a");
        self.emit_load_width(temps[0]);
        self.line("    ld a, (hl)");
        self.line("    cp c");
        self.line(&format!("    jp z, {found}"));
        self.emit_increment_memory(temps[0]);
        self.emit_decrement_memory(temps[1]);
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{found}:"));
        self.line("    ld a, 01h");
        self.line(&format!("    ld ({:06X}h), a", second_destination.addr));
        self.line(&format!("    jp {done}"));
        self.line(&format!("{not_found}:"));
        self.line("    xor a");
        self.line(&format!("    ld ({:06X}h), a", second_destination.addr));
        self.line(&format!("{done}:"));
        self.emit_load_width(temps[0]);
        Ok(())
    }

    fn emit_divmod_variables(
        &mut self,
        dividend: Variable,
        divisor: Variable,
        quotient: Variable,
        remainder: Variable,
        signed: bool,
    ) {
        let zero = self.next_label("intrinsic_div_zero");
        let done = self.next_label("intrinsic_div_done");
        let finished = self.next_label("intrinsic_div_finished");
        let loop_label = self.next_label("intrinsic_div_loop");
        self.emit_zero_bytes(quotient);
        self.emit_zero_bytes(remainder);
        self.emit_jump_if_memory_zero(divisor, &zero);

        if signed {
            let not_overflow = self.next_label("intrinsic_div_not_overflow");
            let quotient_positive = self.next_label("intrinsic_div_q_positive");
            let remainder_positive = self.next_label("intrinsic_div_r_positive");
            let quotient_negative = self.alloc_var(1u32);
            let remainder_negative = self.alloc_var(1u32);
            self.emit_zero_bytes(quotient_negative);
            self.emit_zero_bytes(remainder_negative);
            self.emit_jump_if_memory_not_equals(
                dividend,
                signed_min_bytes_for_size(dividend.size),
                &not_overflow,
            );
            self.emit_jump_if_memory_not_equals(
                divisor,
                signed_negative_one_bytes_for_size(divisor.size),
                &not_overflow,
            );
            self.emit_copy_bytes(dividend, 0, quotient, 0, quotient.size);
            self.emit_zero_bytes(dividend);
            self.line(&format!("    jp {done}"));
            self.line(&format!("{not_overflow}:"));
            self.emit_abs_signed_variable(
                dividend,
                Some(quotient_negative),
                Some(remainder_negative),
            );
            self.emit_abs_signed_variable(divisor, Some(quotient_negative), None);
            self.line(&format!("{loop_label}:"));
            self.emit_compare_memory(dividend, divisor);
            self.line(&format!("    jp c, {done}"));
            self.emit_sub_memory(dividend, divisor);
            self.emit_increment_memory(quotient);
            self.line(&format!("    jp {loop_label}"));
            self.line(&format!("{done}:"));
            self.emit_copy_bytes(dividend, 0, remainder, 0, remainder.size);
            self.emit_load_a(quotient_negative);
            self.line("    or a");
            self.line(&format!("    jp z, {quotient_positive}"));
            self.emit_negate_memory(quotient);
            self.line(&format!("{quotient_positive}:"));
            self.emit_load_a(remainder_negative);
            self.line("    or a");
            self.line(&format!("    jp z, {remainder_positive}"));
            self.emit_negate_memory(remainder);
            self.line(&format!("{remainder_positive}:"));
            self.line(&format!("    jp {finished}"));
        } else {
            self.emit_copy_bytes(dividend, 0, remainder, 0, remainder.size);
            self.line(&format!("{loop_label}:"));
            self.emit_compare_memory(dividend, divisor);
            self.line(&format!("    jp c, {done}"));
            self.emit_sub_memory(dividend, divisor);
            self.emit_increment_memory(quotient);
            self.line(&format!("    jp {loop_label}"));
            self.line(&format!("{done}:"));
            self.emit_copy_bytes(dividend, 0, remainder, 0, remainder.size);
            self.line(&format!("    jp {finished}"));
        }
        self.line(&format!("{zero}:"));
        self.emit_zero_bytes(quotient);
        self.emit_zero_bytes(remainder);
        self.line(&format!("{finished}:"));
    }

    fn emit_two_result_call(
        &mut self,
        name: &str,
        args: &[Expr],
        second_destination: Variable,
    ) -> Result<(), Diagnostic> {
        if builtin_function_arity(name).is_some() {
            return self.emit_two_result_intrinsic_call(name, args, second_destination);
        }
        let sig = self
            .symbols
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if sig.second_return_type.is_none() {
            return Err(Diagnostic::new(format!(
                "function `{name}` does not return two values"
            )));
        }
        if self.current_function_is_interrupt() && !sig.is_interrupt {
            return Err(Diagnostic::new(format!(
                "interrupt function `{}` cannot call non-interrupt function `{name}`",
                self.current_function_name()
            )));
        }
        if sig.arity != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments but got {}",
                sig.arity,
                args.len()
            )));
        }

        let mut temps = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let width = sig.params[index];
            let ty = &sig.param_types[index];
            let temp = self.alloc_var(width.bytes());
            self.emit_expr_to_type(arg, ty)?;
            self.emit_store_width(temp);
            temps.push(temp);
        }

        let pointer_width = self.symbols.type_width(&Type::Named("ptr".to_owned()))?;
        let second_pointer = self.alloc_var(pointer_width.bytes());
        self.line(&format!("    ld hl, {:06X}h", second_destination.addr));
        self.emit_store_width(second_pointer);

        let saved_variables =
            self.recursive_call_saved_variables(name, args, &[second_destination]);
        let return_temp = if saved_variables.is_empty() {
            None
        } else {
            Some(self.alloc_var(sig.return_width.bytes()))
        };
        let hidden_return_arg = second_pointer;

        if sig.uses_arg_slots {
            for (temp, slot) in temps.iter().copied().zip(sig.arg_slots.iter().copied()) {
                self.emit_load_width(temp);
                self.emit_store_width(slot);
            }
            self.emit_save_recursive_call_variables(&saved_variables);
            self.emit_push_stack_arg_variable(hidden_return_arg);
            self.line(&format!("    call {}", function_label(name)));
            self.emit_drop_stack_arg_bytes(sig.stack_arg_bytes);
            self.emit_store_recursive_call_return(return_temp);
            self.emit_restore_recursive_call_variables(&saved_variables);
            self.emit_load_recursive_call_return(return_temp);
            return Ok(());
        }

        self.emit_save_recursive_call_variables(&saved_variables);
        self.emit_push_stack_arg_variable(hidden_return_arg);
        for temp in temps.iter().copied().skip(3).rev() {
            self.emit_push_stack_arg_variable(temp);
        }
        if let Some(temp) = temps.get(2).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld c, a");
            } else if sig.params.get(1).is_some_and(|width| width.bytes() != 1) {
                self.emit_load_width(temp);
                self.line("    push hl");
                self.line("    pop bc");
            } else {
                return Err(Diagnostic::new(
                    "current codegen supports a wide third argument only when the second argument is also wide",
                ));
            }
        }
        if let Some(temp) = temps.get(1).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld b, a");
            } else {
                self.emit_load_width(temp);
                self.line("    ex de, hl");
            }
        }
        if let Some(temp) = temps.first().copied() {
            self.emit_load_width(temp);
        }
        self.line(&format!("    call {}", function_label(name)));
        self.emit_drop_stack_arg_bytes(sig.stack_arg_bytes);
        self.emit_store_recursive_call_return(return_temp);
        self.emit_restore_recursive_call_variables(&saved_variables);
        self.emit_load_recursive_call_return(return_temp);
        Ok(())
    }

    fn emit_indirect_call(&mut self, name: &str, args: &[Expr]) -> Result<(), Diagnostic> {
        if self.current_function_is_interrupt() {
            return Err(Diagnostic::new(format!(
                "interrupt function `{}` cannot call through a function pointer",
                self.current_function_name()
            )));
        }
        let pointer = self.variable(name)?;
        let sig = self.indirect_function_signature(name)?;
        if sig.arity != args.len() {
            return Err(Diagnostic::new(format!(
                "function pointer `{name}` expects {} arguments but got {}",
                sig.arity,
                args.len()
            )));
        }

        let mut temps = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let width = sig.params[index];
            let ty = &sig.param_types[index];
            let temp = self.alloc_var(width.bytes());
            self.emit_expr_to_type(arg, ty)?;
            self.emit_store_width(temp);
            temps.push(temp);
        }

        let saved_variables = self.indirect_call_saved_variables(args, &[]);
        let return_temp = if saved_variables.is_empty() || sig.return_type.is_none() {
            None
        } else {
            Some(self.alloc_var(sig.return_width.bytes()))
        };

        if sig.uses_arg_slots {
            let slots = self
                .symbols
                .function_pointer_arg_slots(&sig.param_types, sig.return_type.as_ref())?;
            for (temp, slot) in temps.iter().copied().zip(slots) {
                self.emit_load_width(temp);
                self.emit_store_width(slot);
            }
        }

        self.emit_save_recursive_call_variables(&saved_variables);
        if sig.stack_arg_bytes > 0 {
            for temp in temps.iter().copied().skip(3).rev() {
                self.emit_push_stack_arg_variable(temp);
            }
        }
        if let Some(temp) = temps.get(2).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld c, a");
            } else if sig.params.get(1).is_some_and(|width| width.bytes() != 1) {
                self.emit_load_width(temp);
                self.line("    push hl");
                self.line("    pop bc");
            } else if !sig.uses_arg_slots {
                return Err(Diagnostic::new(
                    "current codegen supports a wide third argument only when the second argument is also wide",
                ));
            }
        }
        if let Some(temp) = temps.get(1).copied()
            && !sig.uses_arg_slots
        {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld b, a");
            } else {
                self.emit_load_width(temp);
                self.line("    ex de, hl");
            }
        }
        if let Some(temp) = temps.first().copied()
            && !sig.uses_arg_slots
        {
            self.emit_load_width(temp);
        }
        let first_arg_temp = if !sig.uses_arg_slots && !sig.params.is_empty() {
            let temp = self.alloc_var(sig.params[0].bytes());
            if sig.params[0].bytes() == 1 {
                self.emit_store_a(temp);
            } else {
                self.emit_store_width(temp);
            }
            Some(temp)
        } else {
            None
        };

        let helper_label = self.next_label("indirect_call");
        let continuation_label = self.next_label("indirect_call_continue");
        self.line(&format!("    call {helper_label}"));
        self.line(&format!("    jp {continuation_label}"));
        self.line(&format!("{helper_label}:"));
        self.emit_load_width(pointer);
        self.line("    push hl");
        if let Some(first_arg_temp) = first_arg_temp {
            if first_arg_temp.size == 1 {
                self.emit_load_a(first_arg_temp);
            } else {
                self.emit_load_width(first_arg_temp);
            }
        }
        self.line("    ret");
        self.line(&format!("{continuation_label}:"));
        if sig.stack_arg_bytes > 0 {
            self.emit_drop_stack_arg_bytes(sig.stack_arg_bytes);
        }
        self.emit_store_recursive_call_return(return_temp);
        self.emit_restore_recursive_call_variables(&saved_variables);
        self.emit_load_recursive_call_return(return_temp);
        Ok(())
    }

    fn function_value_type(&self, name: &str) -> Result<Type, Diagnostic> {
        let signature = self
            .symbols
            .functions
            .get(name)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if signature.second_return_type.is_some() {
            return Err(Diagnostic::new(format!(
                "function pointer cannot reference two-result function `{name}`"
            )));
        }
        Ok(Type::Function {
            params: signature.param_types.clone(),
            return_type: signature.return_type.clone().map(Box::new),
        })
    }

    fn emit_intel8080_function_pointer(&mut self, name: &str) -> Result<(), Diagnostic> {
        let signature = self
            .symbols
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if signature.second_return_type.is_some() {
            return Err(Diagnostic::new(format!(
                "function pointer cannot reference two-result function `{name}`"
            )));
        }

        let target_label = self.next_label("function_pointer_target");
        let continuation_label = self.next_label("function_pointer_continue");
        let capture_label = self.next_label("function_pointer_capture");
        let after_capture_label = self.next_label("function_pointer_after_capture");

        self.line(&format!("    call {capture_label}"));
        self.line(&format!("{target_label}:"));
        if signature.uses_arg_slots {
            let slots = self.symbols.function_pointer_arg_slots(
                &signature.param_types,
                signature.return_type.as_ref(),
            )?;
            for (source, target) in slots
                .iter()
                .copied()
                .zip(signature.arg_slots.iter().copied())
            {
                self.emit_load_width(source);
                self.emit_store_width(target);
            }
        }
        self.line(&format!("    call {}", function_label(name)));
        self.line("    ret");
        self.line(&format!("{continuation_label}:"));
        self.line(&format!("    jp {after_capture_label}"));
        self.line(&format!("{capture_label}:"));
        self.line("    pop hl");
        self.line(&format!("    jp {continuation_label}"));
        self.line(&format!("{after_capture_label}:"));
        Ok(())
    }

    fn function_pointer_target_label(&self, name: &str) -> Result<String, Diagnostic> {
        let signature = self
            .symbols
            .functions
            .get(name)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if signature.second_return_type.is_some() {
            return Err(Diagnostic::new(format!(
                "function pointer cannot reference two-result function `{name}`"
            )));
        }
        Ok(if signature.uses_arg_slots {
            function_pointer_label(name)
        } else {
            function_label(name)
        })
    }

    fn function_pointer_type(&self, name: &str) -> Result<Type, Diagnostic> {
        let variable_type = self
            .variable_type(name)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{name}`")))?;
        let resolved = self.symbols.resolved_type(variable_type)?;
        let Type::Ptr(inner) = resolved else {
            return Err(Diagnostic::new(format!(
                "function pointer call requires `ptr<fn(...)>`, got `{}`",
                type_display(&resolved)
            )));
        };
        if !matches!(inner.as_ref(), Type::Function { .. }) {
            return Err(Diagnostic::new(format!(
                "function pointer call requires `ptr<fn(...)>`, got `{}`",
                type_display(&Type::Ptr(inner))
            )));
        }
        Ok(*inner)
    }

    fn indirect_function_signature(&self, name: &str) -> Result<FunctionSig, Diagnostic> {
        let Type::Function {
            params,
            return_type,
        } = self.function_pointer_type(name)?
        else {
            unreachable!("function_pointer_type only returns function types")
        };
        let params = params;
        let param_widths = params
            .iter()
            .map(|param| self.symbols.type_width(param))
            .collect::<Result<Vec<_>, _>>()?;
        let uses_arg_slots = param_widths.get(2).is_some_and(|third| third.bytes() != 1)
            && param_widths
                .get(1)
                .is_some_and(|second| second.bytes() == 1);
        let mut stack_arg_offsets = vec![None; params.len()];
        let mut stack_arg_bytes = 0u8;
        let mut stack_offset = self
            .symbols
            .type_width(&Type::Named("ptr".to_owned()))?
            .bytes()
            .saturating_mul(2);
        if !uses_arg_slots && params.len() > 3 {
            for (index, width) in param_widths.iter().enumerate().skip(3) {
                let bytes = width.bytes();
                if stack_offset as u16 + bytes as u16 > 0x80 {
                    return Err(Diagnostic::new(
                        "function pointer stack arguments exceed IX displacement range",
                    ));
                }
                stack_arg_offsets[index] = Some(stack_offset);
                stack_offset += bytes;
                stack_arg_bytes += bytes;
            }
        }
        let return_type = return_type.map(|return_type| *return_type);
        let return_width = return_type
            .as_ref()
            .map(|return_type| self.symbols.type_width(return_type))
            .transpose()?
            .unwrap_or(ValueWidth::U8);
        Ok(FunctionSig {
            arity: params.len(),
            params: param_widths,
            param_types: params,
            arg_slots: Vec::new(),
            uses_arg_slots,
            stack_arg_offsets,
            stack_arg_bytes,
            return_width,
            return_type,
            second_return_width: ValueWidth::U8,
            second_return_type: None,
            hidden_return_arg_offset: None,
            is_interrupt: false,
        })
    }

    fn indirect_call_saved_variables(
        &self,
        args: &[Expr],
        extra_excluded: &[Variable],
    ) -> Vec<Variable> {
        let Some(storage) = self.function_storage_stack.last() else {
            return Vec::new();
        };
        let mut excluded = args
            .iter()
            .filter_map(|arg| match arg {
                Expr::AddressOf(name) => self.variable_opt(name),
                _ => None,
            })
            .collect::<Vec<_>>();
        excluded.extend_from_slice(extra_excluded);
        let mut addresses = storage
            .iter()
            .flat_map(|variable| variable.addr..variable.addr.saturating_add(variable.size))
            .filter(|addr| {
                !excluded.iter().any(|variable| {
                    (variable.addr..variable.addr.saturating_add(variable.size)).contains(addr)
                })
            })
            .collect::<Vec<_>>();
        addresses.sort_unstable();
        addresses.dedup();

        let mut variables: Vec<Variable> = Vec::new();
        for addr in addresses {
            if let Some(variable) = variables.last_mut()
                && variable.addr + variable.size == addr
            {
                variable.size += 1;
            } else {
                variables.push(scalar_var(addr, 1));
            }
        }
        variables
    }

    fn emit_user_call(&mut self, name: &str, args: &[Expr]) -> Result<(), Diagnostic> {
        let sig = self
            .symbols
            .functions
            .get(name)
            .cloned()
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        if self.current_function_is_interrupt() && !sig.is_interrupt {
            return Err(Diagnostic::new(format!(
                "interrupt function `{}` cannot call non-interrupt function `{name}`",
                self.current_function_name()
            )));
        }
        if sig.second_return_type.is_some() {
            return Err(Diagnostic::new(format!(
                "two-result function `{name}` requires a two-destination call"
            )));
        }
        if sig.arity != args.len() {
            return Err(Diagnostic::new(format!(
                "function `{name}` expects {} arguments but got {}",
                sig.arity,
                args.len()
            )));
        }

        let mut temps = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let width = sig.params[index];
            let ty = &sig.param_types[index];
            let temp = self.alloc_var(width.bytes());
            self.emit_expr_to_type(arg, ty)?;
            self.emit_store_width(temp);
            temps.push(temp);
        }

        let saved_variables = self.recursive_call_saved_variables(name, args, &[]);
        let return_temp = if saved_variables.is_empty() || sig.return_type.is_none() {
            None
        } else {
            Some(self.alloc_var(sig.return_width.bytes()))
        };

        if sig.uses_arg_slots {
            for (temp, slot) in temps.iter().copied().zip(sig.arg_slots.iter().copied()) {
                self.emit_load_width(temp);
                self.emit_store_width(slot);
            }
            self.emit_save_recursive_call_variables(&saved_variables);
            self.line(&format!("    call {}", function_label(name)));
            self.emit_store_recursive_call_return(return_temp);
            self.emit_restore_recursive_call_variables(&saved_variables);
            self.emit_load_recursive_call_return(return_temp);
            return Ok(());
        }

        self.emit_save_recursive_call_variables(&saved_variables);
        if sig.stack_arg_bytes > 0 {
            for temp in temps.iter().copied().skip(3).rev() {
                self.emit_push_stack_arg_variable(temp);
            }
        }
        if let Some(temp) = temps.get(2).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld c, a");
            } else if sig.params.get(1).is_some_and(|width| width.bytes() != 1) {
                self.emit_load_width(temp);
                self.line("    push hl");
                self.line("    pop bc");
            } else {
                return Err(Diagnostic::new(
                    "current codegen supports a wide third argument only when the second argument is also wide",
                ));
            }
        }
        if let Some(temp) = temps.get(1).copied() {
            if temp.size == 1 {
                self.emit_load_a(temp);
                self.line("    ld b, a");
            } else {
                self.emit_load_width(temp);
                self.line("    ex de, hl");
            }
        }
        if let Some(temp) = temps.first().copied() {
            self.emit_load_width(temp);
        }
        self.line(&format!("    call {}", function_label(name)));
        if sig.stack_arg_bytes > 0 {
            self.emit_drop_stack_arg_bytes(sig.stack_arg_bytes);
        }
        self.emit_store_recursive_call_return(return_temp);
        self.emit_restore_recursive_call_variables(&saved_variables);
        self.emit_load_recursive_call_return(return_temp);
        Ok(())
    }

    fn recursive_call_saved_variables(
        &self,
        callee: &str,
        args: &[Expr],
        extra_excluded: &[Variable],
    ) -> Vec<Variable> {
        let caller = self.current_function_name();
        if !self
            .recursive_call_edges
            .contains(&(caller.to_owned(), callee.to_owned()))
        {
            return Vec::new();
        }

        let Some(storage) = self.function_storage_stack.last() else {
            return Vec::new();
        };
        let mut excluded = args
            .iter()
            .filter_map(|arg| match arg {
                // The callee can mutate this local through its pointer parameter.
                // Do not restore its pre-call value after returning.
                Expr::AddressOf(name) => self.variable_opt(name),
                _ => None,
            })
            .collect::<Vec<_>>();
        excluded.extend_from_slice(extra_excluded);
        let mut addresses = storage
            .iter()
            .flat_map(|variable| variable.addr..variable.addr.saturating_add(variable.size))
            .filter(|addr| {
                !excluded.iter().any(|variable| {
                    (variable.addr..variable.addr.saturating_add(variable.size)).contains(addr)
                })
            })
            .collect::<Vec<_>>();
        addresses.sort_unstable();
        addresses.dedup();

        let mut variables: Vec<Variable> = Vec::new();
        for addr in addresses {
            if let Some(variable) = variables.last_mut()
                && variable.addr + variable.size == addr
            {
                variable.size += 1;
            } else {
                variables.push(scalar_var(addr, 1));
            }
        }
        variables
    }

    fn emit_save_recursive_call_variables(&mut self, variables: &[Variable]) {
        for variable in variables {
            for offset in 0..variable.size {
                self.line(&format!("    ld a, ({:06X}h)", variable.addr + offset));
                self.line("    dec sp");
                self.line("    ld hl, 000000h");
                self.line("    add hl, sp");
                self.line("    ld (hl), a");
            }
        }
    }

    fn emit_restore_recursive_call_variables(&mut self, variables: &[Variable]) {
        for variable in variables.iter().rev() {
            for offset in (0..variable.size).rev() {
                self.line("    ld hl, 000000h");
                self.line("    add hl, sp");
                self.line("    ld a, (hl)");
                self.line("    inc sp");
                self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
            }
        }
    }

    fn emit_store_recursive_call_return(&mut self, return_temp: Option<Variable>) {
        if let Some(return_temp) = return_temp {
            self.emit_store_width(return_temp);
        }
    }

    fn emit_load_recursive_call_return(&mut self, return_temp: Option<Variable>) {
        if let Some(return_temp) = return_temp {
            self.emit_load_width(return_temp);
        }
    }

    fn call_returns_two_values(&self, name: &str, args: &[Expr]) -> Result<bool, Diagnostic> {
        if CATALOG.lookup(name).is_some() {
            return Ok(self.resolve_intrinsic_call(name, args)?.result_count() == ResultCount::Two);
        }
        let sig = self
            .symbols
            .functions
            .get(name)
            .ok_or_else(|| Diagnostic::new(format!("unknown function `{name}`")))?;
        Ok(sig.second_return_type.is_some())
    }

    fn emit_forward_two_result_call(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        let Some(first_type) = self
            .return_type_stack
            .last()
            .and_then(|return_type| return_type.as_ref())
            .cloned()
        else {
            return Err(Diagnostic::new(format!(
                "function `{}` cannot forward two values without a first return type",
                self.current_function_name()
            )));
        };
        let Some(second_type) = self
            .second_return_type_stack
            .last()
            .and_then(|return_type| return_type.as_ref())
            .cloned()
        else {
            return Err(Diagnostic::new(format!(
                "function `{}` cannot forward two values",
                self.current_function_name()
            )));
        };
        let Some(second_pointer) = self.second_return_pointer_stack.last().copied().flatten()
        else {
            return Err(Diagnostic::new(format!(
                "two-result function `{}` has no caller-provided return slot",
                self.current_function_name()
            )));
        };

        let first_value = self.alloc_var(self.symbols.type_width(&first_type)?.bytes());
        let second_value = self.alloc_var(self.symbols.type_width(&second_type)?.bytes());
        self.emit_two_result_call(name, args, second_value)?;
        self.emit_store_width(first_value);
        self.emit_load_width(second_pointer);
        self.emit_store_var_to_pointed_width(second_value);
        self.emit_load_width(first_value);
        if self.current_function_uses_frame() {
            self.emit_frame_epilogue();
        }
        if self.current_function_is_interrupt() {
            self.emit_interrupt_epilogue();
        } else {
            self.line("    ret");
        }
        Ok(())
    }

    fn emit_return_two(&mut self, first: &Expr, second: &Expr) -> Result<(), Diagnostic> {
        let Some(first_type) = self
            .return_type_stack
            .last()
            .and_then(|return_type| return_type.as_ref())
            .cloned()
        else {
            return Err(Diagnostic::new(format!(
                "function `{}` cannot return two values without a first return type",
                self.current_function_name()
            )));
        };
        let Some(second_type) = self
            .second_return_type_stack
            .last()
            .and_then(|return_type| return_type.as_ref())
            .cloned()
        else {
            return Err(Diagnostic::new(format!(
                "function `{}` cannot return two values",
                self.current_function_name()
            )));
        };
        let Some(second_pointer) = self.second_return_pointer_stack.last().copied().flatten()
        else {
            return Err(Diagnostic::new(format!(
                "two-result function `{}` has no caller-provided return slot",
                self.current_function_name()
            )));
        };

        let first_value = self.alloc_var(self.symbols.type_width(&first_type)?.bytes());
        let second_value = self.alloc_var(self.symbols.type_width(&second_type)?.bytes());
        self.emit_expr_to_type(first, &first_type)?;
        self.emit_store_width(first_value);
        self.emit_expr_to_type(second, &second_type)?;
        self.emit_store_width(second_value);
        self.emit_load_width(second_pointer);
        self.emit_store_var_to_pointed_width(second_value);
        self.emit_load_width(first_value);
        if self.current_function_uses_frame() {
            self.emit_frame_epilogue();
        }
        if self.current_function_is_interrupt() {
            self.emit_interrupt_epilogue();
        } else {
            self.line("    ret");
        }
        Ok(())
    }

    fn emit_expr_to_width(&mut self, expr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        match width {
            ValueWidth::U8 => self.emit_expr_to_a(expr),
            ValueWidth::U16 | ValueWidth::U24 => self.emit_expr_to_hl(expr, width),
        }
    }

    fn emit_expr_to_type(&mut self, expr: &Expr, ty: &Type) -> Result<(), Diagnostic> {
        let width = self.symbols.type_width(ty)?;
        self.validate_expr_arithmetic_compatibility(expr)?;
        self.validate_expr_assignable_to_type(expr, ty)?;
        if let Expr::Cast { expr, ty } = expr {
            self.emit_cast_to_type(expr, ty)?;
            return Ok(());
        }
        if !self.is_pointer_arithmetic_expr(expr)?
            && let Ok(value) = self.eval_i64_with_local_constants(expr)
        {
            let value = self.value_for_type(value, ty, width)?;
            match width {
                ValueWidth::U8 => self.line(&format!("    ld a, {value:02X}h")),
                ValueWidth::U16 | ValueWidth::U24 => self.line(&format!("    ld hl, {value:06X}h")),
            }
            return Ok(());
        }
        self.emit_expr_to_width(expr, width)
    }

    fn is_pointer_arithmetic_expr(&self, expr: &Expr) -> Result<bool, Diagnostic> {
        if let Expr::Binary { left, op, right } = expr {
            return Ok(matches!(op, BinaryOp::Add | BinaryOp::Sub)
                && (self.pointer_pointee_size(left)?.is_some()
                    || self.pointer_pointee_size(right)?.is_some()));
        }
        Ok(false)
    }

    fn emit_cast_to_type(&mut self, expr: &Expr, ty: &Type) -> Result<(), Diagnostic> {
        self.validate_cast(expr, ty)?;
        let width = self.symbols.type_width(ty)?;
        let target_type = self.symbols.resolved_type(ty)?;
        let source_type = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if !self.is_pointer_arithmetic_expr(expr)?
            && let Ok(value) = self.eval_i64_with_local_constants(expr)
        {
            let bits = u32::from(width.bytes()) * 8;
            let mask = (1_i128 << bits) - 1;
            let value = if type_is_bool(&target_type) {
                u32::from(value != 0)
            } else {
                ((value as i128) & mask) as u32
            };
            match width {
                ValueWidth::U8 => self.line(&format!("    ld a, {value:02X}h")),
                ValueWidth::U16 | ValueWidth::U24 => self.line(&format!("    ld hl, {value:06X}h")),
            }
            return Ok(());
        }
        let source_width = self.expr_width(expr)?;
        match width {
            ValueWidth::U8 => {
                if type_is_bool(&target_type) {
                    if source_width == ValueWidth::U8 {
                        self.emit_expr_to_a(expr)?;
                        self.emit_normalize_a_to_bool();
                    } else {
                        self.emit_expr_to_hl(expr, source_width)?;
                        self.emit_normalize_hl_to_bool(source_width);
                    }
                } else if source_width == ValueWidth::U8 {
                    self.emit_expr_to_a(expr)?;
                } else {
                    self.emit_expr_to_hl(expr, source_width)?;
                    self.line("    ld a, l");
                }
            }
            ValueWidth::U16 => {
                if source_width == ValueWidth::U8 {
                    self.emit_expr_to_a(expr)?;
                    self.line("    ld hl, 000000h");
                    self.line("    ld l, a");
                    self.emit_sign_extend_widened_integer(&source_type, source_width, width)?;
                } else {
                    self.emit_expr_to_hl(expr, source_width)?;
                    self.zero_extend_hl16();
                }
            }
            ValueWidth::U24 => {
                if source_width == ValueWidth::U8 {
                    self.emit_expr_to_a(expr)?;
                    self.line("    ld hl, 000000h");
                    self.line("    ld l, a");
                    self.emit_sign_extend_widened_integer(&source_type, source_width, width)?;
                } else {
                    self.emit_expr_to_hl(expr, source_width)?;
                    self.emit_sign_extend_widened_integer(&source_type, source_width, width)?;
                }
            }
        }
        Ok(())
    }

    fn emit_normalize_a_to_bool(&mut self) {
        let true_label = self.next_label("cast_bool_true");
        let end_label = self.next_label("cast_bool_end");
        self.line("    or a");
        self.line(&format!("    jp nz, {true_label}"));
        self.line("    ld a, 00h");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{true_label}:"));
        self.line("    ld a, 01h");
        self.line(&format!("{end_label}:"));
    }

    fn emit_normalize_hl_to_bool(&mut self, width: ValueWidth) {
        let value = self.alloc_var(width.bytes());
        self.emit_store_width(value);
        self.line("    xor a");
        for offset in 0..width.bytes() {
            self.line("    ld b, a");
            self.line(&format!("    ld a, ({:06X}h)", value.addr + offset as u32));
            self.line("    or b");
        }
        self.emit_normalize_a_to_bool();
    }

    fn emit_sign_extend_widened_integer(
        &mut self,
        source_type: &Type,
        source_width: ValueWidth,
        target_width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if source_width >= target_width || !type_is_signed(source_type) {
            return Ok(());
        }
        let (sign_register, extension) = match source_width {
            ValueWidth::U8 => ("l", 0xFFFF00),
            ValueWidth::U16 => ("h", 0xFF0000),
            ValueWidth::U24 => return Ok(()),
        };
        let done = self.next_label("cast_nonnegative");
        self.line(&format!("    ld a, {sign_register}"));
        self.line("    cp 80h");
        self.line(&format!("    jp c, {done}"));
        self.emit_add_hl_const(extension);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn validate_cast(&self, expr: &Expr, target: &Type) -> Result<(), Diagnostic> {
        let source_type = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        let target_type = self.symbols.resolved_type(target)?;
        match (&source_type, &target_type) {
            (Type::Ptr(_), Type::Ptr(_)) => Ok(()),
            (Type::Ptr(_), Type::Named(name)) if name == "bool" => Ok(()),
            (Type::Ptr(_), Type::Named(name))
                if is_raw_address_type(name)
                    || (name == "u16" && self.cpu_uses_16_bit_pointers()) =>
            {
                Ok(())
            }
            (Type::Ptr(_), Type::Named(_)) => Err(Diagnostic::new(
                "pointer-to-integer casts produce u24 or ptr",
            )),
            (Type::Named(name), Type::Ptr(_)) if is_raw_address_type(name) => Ok(()),
            (Type::Named(_), Type::Ptr(_)) => Err(Diagnostic::new(
                "integer-to-pointer casts require u24 or ptr",
            )),
            _ => Ok(()),
        }
    }

    fn cpu_uses_16_bit_pointers(&self) -> bool {
        is_z80_family_16bit(self.cpu)
    }

    fn emit_expr_to_hl(&mut self, expr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(value) = self.local_constant_value_for_width(name, width)? {
                    self.line(&format!("    ld hl, {value:06X}h"));
                } else if let Some(variable) = self.variable_opt(name) {
                    if variable.size == 1 {
                        self.emit_load_a(variable);
                        self.line("    ld hl, 000000h");
                        self.line("    ld l, a");
                    } else if variable.size == 2 {
                        self.emit_load_hl16(variable);
                    } else {
                        self.emit_load_hl(variable);
                    }
                } else {
                    let value = self.value_for_width(expr, width)?;
                    self.line(&format!("    ld hl, {:06X}h", value));
                }
            }
            Expr::AddressOfIndex { name, index } => {
                self.emit_array_element_address(name, index)?;
            }
            Expr::AddressOfField { base, field } => {
                self.emit_field_address(base, field)?;
            }
            Expr::AddressOfAccess(path) => {
                let path = self.canonical_access_path(path);
                self.emit_access_address(&path)?;
            }
            Expr::AddressOf(name) => {
                if self.symbols.functions.contains_key(name) {
                    if is_intel_8080_family(self.cpu) {
                        self.emit_intel8080_function_pointer(name)?;
                    } else {
                        let label = self.function_pointer_target_label(name)?;
                        self.line(&format!("    ld hl, {label}"));
                    }
                } else {
                    self.emit_variable_address(name)?;
                }
            }
            Expr::String(value) => {
                self.emit_string_literal_address(value)?;
            }
            Expr::Deref(ptr) => {
                self.emit_deref_to_hl(ptr, width)?;
            }
            Expr::BankedPointer { pointer, .. } => self.emit_expr_to_hl(pointer, width)?,
            Expr::Field { base, field } => {
                if self.emit_dotted_constant_to_hl(base, field, width)? {
                    return Ok(());
                }
                if let Some(variable) = self.dotted_variable(base, field) {
                    self.emit_load_width(variable);
                    return Ok(());
                }
                let variable = self.field_variable(base, field)?;
                self.emit_load_width(variable);
            }
            Expr::Index { name, index } => {
                self.emit_load_indexed_element_to_hl(name, index)?;
            }
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty()
                    && (self.named_value_type(&path.root).is_some()
                        || self.symbols.embed_property_value(&path.root).is_some())
                {
                    let value = self.value_for_width(&Expr::Ident(path.root.clone()), width)?;
                    self.line(&format!("    ld hl, {:06X}h", value));
                    return Ok(());
                }
                let ty = self.access_type(&path)?;
                let size = self.symbols.type_size(&ty)?;
                if size > 3 {
                    return Err(Diagnostic::new(format!(
                        "value `{}` is not scalar-sized",
                        access_path_summary(&path)
                    )));
                }
                if let Some(variable) = self.const_access_variable(&path)? {
                    self.emit_load_width(variable);
                    return Ok(());
                }
                self.emit_access_address(&path)?;
                let stored = self.alloc_var(size);
                self.emit_load_pointed_width_into(stored);
                self.emit_load_width(stored);
            }
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Char(_) | Expr::Bool(_) => {
                let value = self.value_for_width(expr, width)?;
                self.line(&format!("    ld hl, {:06X}h", value));
            }
            Expr::Cast { expr, ty } => self.emit_cast_to_type(expr, ty)?,
            Expr::Unary { op, expr } => {
                if *op == UnaryOp::Not {
                    self.ensure_expr_is_bool(expr, "logical operand")?;
                }
                self.emit_unary_to_hl(*op, expr, width)?
            }
            Expr::Binary { left, op, right } => match op {
                BinaryOp::Add | BinaryOp::Sub
                    if self.emit_pointer_arithmetic(left, *op, right)? =>
                {
                    return Ok(());
                }
                BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor => {
                    self.ensure_binary_arithmetic_operands_compatible(left, right)?;
                    if self.emit_single_bit_mask(left, *op, right, width)? {
                        return Ok(());
                    }
                    if self.emit_u16_immediate_bitwise(left, *op, right, width)? {
                        return Ok(());
                    }
                    if *op == BinaryOp::Mul {
                        self.emit_mul_to_width(
                            left,
                            right,
                            width,
                            self.binary_operands_are_signed(left, right)?,
                        )?;
                        return Ok(());
                    }
                    self.emit_expr_to_hl(left, width)?;
                    self.line("    push hl");
                    self.emit_expr_to_hl(right, width)?;
                    self.line("    pop bc");
                    self.emit_wide_op_with_left_in_bc(*op, width)?;
                }
                BinaryOp::Shl | BinaryOp::Shr => {
                    self.ensure_shift_operands_compatible(left, right)?;
                    let temp = self.alloc_var(width.bytes());
                    let signed = self.expr_is_signed(left)?;
                    self.emit_expr_to_hl(left, width)?;
                    self.emit_store_width(temp);
                    self.emit_shift_temporary_by_expr(temp, *op, right, signed)?;
                    self.emit_load_width(temp);
                }
                BinaryOp::Div | BinaryOp::Mod => {
                    self.ensure_binary_arithmetic_operands_compatible(left, right)?;
                    if self.binary_operands_are_signed(left, right)? {
                        self.emit_signed_div_mod_to_width(left, right, *op, width)?;
                    } else {
                        self.emit_div_mod_to_width(left, right, *op, width)?;
                    }
                    return Ok(());
                }
                _ => {
                    return Err(Diagnostic::new(format!(
                        "binary operator `{op:?}` is not implemented in wide codegen yet"
                    )));
                }
            },
            Expr::Call { path, args } if CATALOG.lookup(&path_text(path)).is_some() => {
                self.emit_intrinsic_value(&path_text(path), args)?;
            }
            Expr::Call { path, args } => {
                self.emit_callable_call(&path_text(path), args)?;
            }
            Expr::Array(_) | Expr::StructInit { .. } | Expr::In(_) => {
                return Err(Diagnostic::new(format!(
                    "expression `{expr:?}` is not supported in u16 codegen"
                )));
            }
        }
        if width == ValueWidth::U16 {
            self.zero_extend_hl16();
        }
        Ok(())
    }

    fn zero_extend_hl16(&mut self) {
        let temp = self.alloc_var(ValueWidth::U16.bytes());
        self.emit_store_hl16(temp);
        self.emit_load_hl16(temp);
    }

    fn emit_pointer_arithmetic(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<bool, Diagnostic> {
        let left_scale = self.pointer_pointee_size(left)?;
        let right_scale = self.pointer_pointee_size(right)?;
        match (op, left_scale, right_scale) {
            (BinaryOp::Add, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer arithmetic requires exactly one pointer operand",
            )),
            (BinaryOp::Add, Some(scale), None) => {
                self.ensure_pointer_offset_expr(right)?;
                self.emit_expr_to_hl(left, ValueWidth::U24)?;
                if self.emit_small_byte_pointer_offset(scale, right, false)? {
                    return Ok(true);
                }
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(right, scale)?;
                self.line("    pop bc");
                self.line("    add hl, bc");
                Ok(true)
            }
            (BinaryOp::Add, None, Some(scale)) => {
                self.ensure_pointer_offset_expr(left)?;
                self.emit_expr_to_hl(right, ValueWidth::U24)?;
                if self.emit_small_byte_pointer_offset(scale, left, false)? {
                    return Ok(true);
                }
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(left, scale)?;
                self.line("    pop bc");
                self.line("    add hl, bc");
                Ok(true)
            }
            (BinaryOp::Sub, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer subtraction between two pointers is not supported",
            )),
            (BinaryOp::Sub, Some(scale), None) => {
                self.ensure_pointer_offset_expr(right)?;
                self.emit_expr_to_hl(left, ValueWidth::U24)?;
                if self.emit_small_byte_pointer_offset(scale, right, true)? {
                    return Ok(true);
                }
                self.line("    push hl");
                self.emit_scaled_offset_to_hl(right, scale)?;
                self.line("    ex de, hl");
                self.line("    pop hl");
                self.line("    or a");
                self.line("    sbc hl, de");
                Ok(true)
            }
            (BinaryOp::Sub, None, Some(_)) => Err(Diagnostic::new(
                "cannot subtract a pointer from a non-pointer value",
            )),
            _ => Ok(false),
        }
    }

    fn emit_small_byte_pointer_offset(
        &mut self,
        scale: u32,
        offset: &Expr,
        subtract: bool,
    ) -> Result<bool, Diagnostic> {
        if scale != 1 {
            return Ok(false);
        }
        let Ok(offset) = self.eval_i64_with_local_constants(offset) else {
            return Ok(false);
        };
        let signed_offset = if subtract { -offset } else { offset };
        if !(-8..=8).contains(&signed_offset) {
            return Ok(false);
        }
        for _ in 0..signed_offset.unsigned_abs() {
            self.line(if signed_offset < 0 {
                "    dec hl"
            } else {
                "    inc hl"
            });
        }
        Ok(true)
    }

    fn ensure_pointer_offset_expr(&self, expr: &Expr) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if type_is_bool(&ty) || matches!(ty, Type::Ptr(_) | Type::Function { .. }) {
            return Err(Diagnostic::new(
                "pointer arithmetic offset must be an integer",
            ));
        }
        self.symbols.type_width(&ty)?;
        Ok(())
    }

    fn emit_scaled_offset_to_hl(&mut self, expr: &Expr, scale: u32) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(expr, ValueWidth::U24)?;
        self.emit_sign_extend_pointer_offset(expr)?;
        match scale {
            1 => {}
            _ => {
                let base = self.alloc_var(ValueWidth::U24.bytes());
                self.emit_store_width(base);
                self.line("    ld hl, 000000h");
                for _ in 0..scale {
                    self.line("    push hl");
                    self.emit_load_width(base);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                }
            }
        }
        Ok(())
    }

    fn emit_sign_extend_pointer_offset(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        if !self.expr_is_signed(expr)? {
            return Ok(());
        }
        let width = self.symbols.type_width(&self.expr_type(expr)?)?;
        let (sign_register, extension) = match width {
            ValueWidth::U8 => ("l", 0xFFFF00),
            ValueWidth::U16 => ("h", 0xFF0000),
            ValueWidth::U24 => return Ok(()),
        };
        let done = self.next_label("offset_nonnegative");
        self.line(&format!("    ld a, {sign_register}"));
        self.line("    cp 80h");
        self.line(&format!("    jp c, {done}"));
        self.emit_add_hl_const(extension);
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_wide_op_with_left_in_bc(
        &mut self,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        match op {
            BinaryOp::Add => {
                self.line("    add hl, bc");
            }
            BinaryOp::Sub => {
                self.line("    ex de, hl");
                self.line("    push bc");
                self.line("    pop hl");
                self.line("    or a");
                self.line("    sbc hl, de");
            }
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => {
                self.emit_wide_bitwise_from_bc_hl(op, width)?;
            }
            _ => unreachable!("unsupported wide op"),
        }
        Ok(())
    }

    fn emit_wide_bitwise_from_bc_hl(
        &mut self,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        let right = self.alloc_var(width.bytes());
        self.emit_store_width(right);
        self.line("    push bc");
        self.line("    pop hl");
        let left = self.alloc_var(width.bytes());
        self.emit_store_width(left);
        let result = self.alloc_var(width.bytes());

        for offset in 0..width.bytes() {
            self.line(&format!("    ld a, ({:06X}h)", left.addr + offset as u32));
            self.line("    ld b, a");
            self.line(&format!("    ld a, ({:06X}h)", right.addr + offset as u32));
            match op {
                BinaryOp::BitAnd => self.line("    and b"),
                BinaryOp::BitOr => self.line("    or b"),
                BinaryOp::BitXor => self.line("    xor b"),
                _ => unreachable!("not a bitwise op"),
            }
            self.line(&format!("    ld ({:06X}h), a", result.addr + offset as u32));
        }

        self.emit_load_width(result);
        Ok(())
    }

    fn emit_expr_to_a(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(value) = self.local_constant_value_for_width(name, ValueWidth::U8)? {
                    self.line(&format!("    ld a, {value:02X}h"));
                } else if let Some(variable) = self.variable_opt(name) {
                    self.emit_load_a(variable);
                } else {
                    let value = self.u8(expr)?;
                    self.line(&format!("    ld a, {:02X}h", value));
                }
            }
            Expr::In(port) => {
                let port = self.port(port)?;
                self.emit_in_a(port);
            }
            Expr::Index { name, index } => {
                self.emit_load_indexed_element_to_a(name, index)?;
            }
            Expr::Field { base, field } => {
                if self.emit_dotted_constant_to_a(base, field)? {
                    return Ok(());
                }
                if let Some(variable) = self.dotted_variable(base, field) {
                    if variable.size != 1 {
                        return Err(Diagnostic::new(format!(
                            "value `{base}.{field}` is not u8-sized"
                        )));
                    }
                    self.emit_load_a(variable);
                    return Ok(());
                }
                let variable = self.field_variable(base, field)?;
                if variable.size != 1 {
                    return Err(Diagnostic::new(format!(
                        "field `{base}.{field}` is not u8-sized"
                    )));
                }
                self.emit_load_a(variable);
            }
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty()
                    && (self.named_value_type(&path.root).is_some()
                        || self.symbols.embed_property_value(&path.root).is_some())
                {
                    let value = self.u8(&Expr::Ident(path.root.clone()))?;
                    self.line(&format!("    ld a, {:02X}h", value));
                    return Ok(());
                }
                let ty = self.access_type(&path)?;
                let size = self.symbols.type_size(&ty)?;
                if size != 1 {
                    return Err(Diagnostic::new(format!(
                        "value `{}` is not u8-sized",
                        access_path_summary(&path)
                    )));
                }
                if let Some(variable) = self.const_access_variable(&path)? {
                    self.emit_load_a(variable);
                    return Ok(());
                }
                self.emit_access_address(&path)?;
                self.line("    ld a, (hl)");
            }
            Expr::Deref(ptr) => {
                self.emit_deref_to_a(ptr)?;
            }
            Expr::BankedPointer { pointer, .. } => self.emit_expr_to_a(pointer)?,
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Char(_) | Expr::Bool(_) => {
                let value = self.value_for_width(expr, ValueWidth::U8)?;
                self.line(&format!("    ld a, {:02X}h", value));
            }
            Expr::Cast { expr, ty } => self.emit_cast_to_type(expr, ty)?,
            Expr::Unary { op, expr } => {
                if *op == UnaryOp::Not {
                    self.ensure_expr_is_bool(expr, "logical operand")?;
                }
                self.emit_unary_to_a(*op, expr)?
            }
            Expr::Binary { left, op, right } => self.emit_binary_expr(left, *op, right)?,
            Expr::Call { path, args } if CATALOG.lookup(&path_text(path)).is_some() => {
                self.emit_intrinsic_value(&path_text(path), args)?;
            }
            Expr::Call { path, args } => {
                self.emit_callable_call(&path_text(path), args)?;
            }
            Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_)
            | Expr::AddressOf(_)
            | Expr::Array(_)
            | Expr::StructInit { .. }
            | Expr::String(_) => {
                return Err(Diagnostic::new(format!(
                    "expression `{expr:?}` is not supported in u8 codegen"
                )));
            }
        }
        Ok(())
    }

    fn emit_mem_peek8(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 1 {
            return Err(Diagnostic::new("mem.peek8 requires one argument"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.line("    ld a, (hl)");
        Ok(())
    }

    fn emit_mem_poke8(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 2 {
            return Err(Diagnostic::new("mem.poke8 requires two arguments"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.validate_expr_assignable_to_type(&args[1], &Type::Named("u8".to_owned()))?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        let value = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(addr);
        self.emit_expr_to_a(&args[1])?;
        self.emit_store_a(value);
        self.emit_load_hl(addr);
        self.emit_load_a(value);
        self.line("    ld (hl), a");
        Ok(())
    }

    fn emit_memcpy(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 3 {
            return Err(Diagnostic::new("mem.memcpy requires three arguments"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.validate_expr_is_ptr_u8(&args[1])?;
        self.validate_expr_assignable_to_type(&args[2], &Type::Named("u24".to_owned()))?;
        let dst = self.alloc_var(ValueWidth::U24.bytes());
        let src = self.alloc_var(ValueWidth::U24.bytes());
        let len = self.alloc_var(ValueWidth::U24.bytes());

        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(dst);
        self.emit_expr_to_hl(&args[1], ValueWidth::U24)?;
        self.emit_store_hl(src);
        self.emit_expr_to_hl(&args[2], ValueWidth::U24)?;
        self.emit_store_hl(len);

        self.emit_load_hl(len);
        self.line("    push hl");
        self.line("    pop bc");
        self.emit_load_hl(src);
        self.line("    ex de, hl");
        self.emit_load_hl(dst);
        self.line("    call __ezra_memcpy");
        Ok(())
    }

    fn emit_memset(&mut self, args: &[Expr]) -> Result<(), Diagnostic> {
        if args.len() != 3 {
            return Err(Diagnostic::new("mem.memset requires three arguments"));
        }
        self.validate_expr_is_ptr_u8(&args[0])?;
        self.validate_expr_assignable_to_type(&args[1], &Type::Named("u8".to_owned()))?;
        self.validate_expr_assignable_to_type(&args[2], &Type::Named("u24".to_owned()))?;
        let dst = self.alloc_var(ValueWidth::U24.bytes());
        let value = self.alloc_var(ValueWidth::U8.bytes());
        let len = self.alloc_var(ValueWidth::U24.bytes());

        self.emit_expr_to_hl(&args[0], ValueWidth::U24)?;
        self.emit_store_hl(dst);
        self.emit_expr_to_a(&args[1])?;
        self.emit_store_a(value);
        self.emit_expr_to_hl(&args[2], ValueWidth::U24)?;
        self.emit_store_hl(len);

        self.emit_load_hl(len);
        self.line("    push hl");
        self.line("    pop bc");
        self.emit_load_a(value);
        self.emit_load_hl(dst);
        self.line("    call __ezra_memset");
        Ok(())
    }

    fn emit_dotted_constant_to_hl(
        &mut self,
        base: &str,
        field: &str,
        width: ValueWidth,
    ) -> Result<bool, Diagnostic> {
        let key = format!("{base}.{field}");
        if !self.symbols.constants.contains_key(&key) {
            return Ok(false);
        }
        let value = self.value_for_width(&Expr::Ident(key), width)?;
        self.line(&format!("    ld hl, {value:06X}h"));
        Ok(true)
    }

    fn emit_dotted_constant_to_a(&mut self, base: &str, field: &str) -> Result<bool, Diagnostic> {
        let key = format!("{base}.{field}");
        if !self.symbols.constants.contains_key(&key) {
            return Ok(false);
        }
        let value = self.u8(&Expr::Ident(key))?;
        self.line(&format!("    ld a, {value:02X}h"));
        Ok(true)
    }

    fn emit_string_literal_address(&mut self, value: &str) -> Result<(), Diagnostic> {
        if let Some(variable) = self.string_literals.get(value).copied() {
            self.line(&format!("    ld hl, {:06X}h", variable.addr));
            return Ok(());
        }

        let variable = self.symbols.intern_string_literal(value)?;
        self.emit_string_literal_initializer(value, variable);
        self.string_literals.insert(value.to_owned(), variable);
        self.line(&format!("    ld hl, {:06X}h", variable.addr));
        Ok(())
    }

    fn emit_deref_to_a(&mut self, ptr: &Expr) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        self.line("    ld a, (hl)");
        Ok(())
    }

    fn emit_deref_to_hl(&mut self, ptr: &Expr, width: ValueWidth) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        match width {
            ValueWidth::U8 => {
                self.line("    ld a, (hl)");
                self.line("    ld hl, 000000h");
                self.line("    ld l, a");
            }
            ValueWidth::U16 | ValueWidth::U24 => {
                let result = self.alloc_var(width.bytes());
                for offset in 0..width.bytes() {
                    if offset != 0 {
                        self.line("    inc hl");
                    }
                    self.line("    ld a, (hl)");
                    self.line(&format!("    ld ({:06X}h), a", result.addr + offset as u32));
                }
                self.emit_load_width(result);
            }
        }
        Ok(())
    }

    fn emit_deref_assignment(
        &mut self,
        ptr: &Expr,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let pointee_type = match self.symbols.resolved_type(&self.expr_type(ptr)?)? {
            Type::Ptr(inner) => *inner,
            Type::Named(name) if name == "ptr" => {
                return Err(Diagnostic::new(
                    "raw ptr dereference requires an explicit typed pointer cast",
                ));
            }
            other => {
                return Err(Diagnostic::new(format!(
                    "cannot assign through non-pointer expression of type `{other:?}`"
                )));
            }
        };
        self.ensure_pointer_write_target_is_mutable(ptr, &pointee_type)?;
        if op == AssignOp::Add
            && self.symbols.type_size(&pointee_type)? == 1
            && is_unit_integer_literal(value)
        {
            self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
            self.line("    ld a, (hl)");
            self.line("    inc a");
            self.line("    ld (hl), a");
            return Ok(());
        }
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        self.emit_store_hl(addr);

        if op != AssignOp::Set {
            let width = self.symbols.type_width(&pointee_type)?;
            let current = self.alloc_var(width.bytes());
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.alloc_var(width.bytes());
            let signed = self.type_is_signed(&pointee_type)?;
            self.emit_typed_assignment_value(current, &pointee_type, op, value, signed)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        self.validate_expr_assignable_to_type(value, &pointee_type)?;
        let stored = self.alloc_storage(&pointee_type)?;
        self.emit_storage_initializer(stored, &pointee_type, value)?;
        self.emit_load_hl(addr);
        self.emit_store_var_to_pointed_width(stored);
        Ok(())
    }

    fn ensure_pointer_write_target_is_mutable(
        &mut self,
        ptr: &Expr,
        pointee_type: &Type,
    ) -> Result<(), Diagnostic> {
        let Some(addr) = self.readonly_write_addr(ptr) else {
            return Ok(());
        };
        let size = u64::from(self.symbols.type_size(pointee_type)?);
        let write_start = u64::from(addr);
        let write_end = write_start.saturating_add(size);
        for (name, embed) in &self.symbols.embeds {
            let Some(len) = embed.variable.len else {
                continue;
            };
            if len == 0 {
                continue;
            }
            let embed_start = u64::from(embed.variable.addr);
            let embed_end = embed_start + u64::from(len);
            if write_start < embed_end && write_end > embed_start {
                return Err(Diagnostic::new(format!(
                    "embedded object `{name}` is read-only"
                )));
            }
        }
        if self
            .readonly_string_literal_for_range(write_start, write_end)
            .is_some()
        {
            return Err(Diagnostic::new("string literal is read-only"));
        }
        Ok(())
    }

    fn readonly_write_addr(&mut self, ptr: &Expr) -> Option<u32> {
        if let Some(addr) = self.readonly_expr_addr(ptr) {
            return Some(addr);
        }
        let Ok(addr) = self.symbols.eval_i64(ptr) else {
            return None;
        };
        Self::addr24(addr)
    }

    fn readonly_expr_addr(&mut self, expr: &Expr) -> Option<u32> {
        match expr {
            Expr::Ident(name) => self.readonly_pointer_alias(name).or_else(|| {
                self.symbols
                    .readonly_global_pointer_aliases
                    .get(name)
                    .copied()
            }),
            Expr::String(value) => {
                if let Some(variable) = self
                    .string_literals
                    .get(value)
                    .or_else(|| self.symbols.string_literals.get(value))
                {
                    return Some(variable.addr);
                }
                let variable = self.symbols.intern_string_literal(value).ok()?;
                self.string_literals.insert(value.clone(), variable);
                Some(variable.addr)
            }
            Expr::Cast { expr, .. } => self.readonly_expr_addr(expr),
            Expr::Binary {
                left,
                op: op @ (BinaryOp::Add | BinaryOp::Sub),
                right,
            } => {
                let base = self.readonly_expr_addr(left)?;
                let Type::Ptr(inner) = self
                    .expr_type(left)
                    .ok()
                    .and_then(|ty| self.symbols.resolved_type(&ty).ok())?
                else {
                    return None;
                };
                let offset = self.eval_i64_with_local_constants(right).ok()?;
                let offset = if *op == BinaryOp::Sub {
                    offset.wrapping_neg()
                } else {
                    offset
                };
                let scale = i64::from(self.symbols.type_size(&inner).ok()?);
                Self::addr24(i64::from(base).wrapping_add(offset.wrapping_mul(scale)))
            }
            _ => None,
        }
    }

    fn addr24(addr: i64) -> Option<u32> {
        if (0..=0xFF_FFFF).contains(&addr) {
            Some(addr as u32)
        } else {
            None
        }
    }

    fn emit_binary_expr(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        if matches!(op, BinaryOp::And | BinaryOp::Or) {
            self.emit_short_circuit_logical(left, op, right)?;
            return Ok(());
        }
        if is_comparison(op) {
            self.ensure_comparison_operands_compatible(left, op, right)?;
            let width = self.expr_width(left)?.max(self.expr_width(right)?);
            if self.binary_operands_are_signed(left, right)? {
                self.emit_signed_comparison(left, op, right, width)?;
                return Ok(());
            }
            if width != ValueWidth::U8 {
                self.emit_wide_comparison(left, op, right, width)?;
                return Ok(());
            }
        }
        if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
            self.ensure_shift_operands_compatible(left, right)?;
            let signed = self.expr_is_signed(left)?;
            self.emit_expr_to_a(left)?;
            self.emit_shift_a_by_expr(op, right, signed)?;
            return Ok(());
        }
        if matches!(op, BinaryOp::Div | BinaryOp::Mod) {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
            if self.binary_operands_are_signed(left, right)? {
                self.emit_signed_div_mod_to_width(left, right, op, ValueWidth::U8)?;
            } else {
                self.emit_u8_div_mod(left, right, op)?;
            }
            return Ok(());
        }
        if op == BinaryOp::Mul {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
            self.emit_mul_to_width(
                left,
                right,
                ValueWidth::U8,
                self.binary_operands_are_signed(left, right)?,
            )?;
            return Ok(());
        }
        if matches!(op, BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor) {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
            if self.emit_single_bit_mask(left, op, right, ValueWidth::U8)? {
                return Ok(());
            }
        }

        if matches!(
            op,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor
        ) {
            self.ensure_binary_arithmetic_operands_compatible(left, right)?;
        }
        let left_var = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_expr_to_a(left)?;
        self.emit_store_a(left_var);
        self.emit_expr_to_a(right)?;
        self.line("    ld c, a");
        self.emit_load_a(left_var);
        match op {
            BinaryOp::Add => self.line("    add a, c"),
            BinaryOp::Sub => self.line("    sub c"),
            BinaryOp::BitAnd => self.line("    and c"),
            BinaryOp::BitOr => self.line("    or c"),
            BinaryOp::BitXor => self.line("    xor c"),
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => self.emit_comparison(op),
            BinaryOp::And | BinaryOp::Or => unreachable!("logical ops handled before binary load"),
            BinaryOp::Div | BinaryOp::Mod | BinaryOp::Shl | BinaryOp::Shr => {
                return Err(Diagnostic::new(format!(
                    "binary operator `{op:?}` is not implemented in u8 codegen yet"
                )));
            }
            BinaryOp::Mul => unreachable!("multiplication handled before u8 binary dispatch"),
        }
        Ok(())
    }

    fn emit_short_circuit_logical(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        self.ensure_expr_is_bool(left, "logical operand")?;
        self.ensure_expr_is_bool(right, "logical operand")?;
        let short_label = self.next_label("logical_short");
        let end_label = self.next_label("logical_end");

        self.emit_expr_to_a(left)?;
        self.line("    or a");
        match op {
            BinaryOp::And => {
                self.line(&format!("    jp z, {short_label}"));
                self.emit_expr_to_a(right)?;
                self.line("    or a");
                self.line(&format!("    jp z, {short_label}"));
                self.line("    ld a, 01h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{short_label}:"));
                self.line("    ld a, 00h");
            }
            BinaryOp::Or => {
                self.line(&format!("    jp nz, {short_label}"));
                self.emit_expr_to_a(right)?;
                self.line("    or a");
                self.line(&format!("    jp nz, {short_label}"));
                self.line("    ld a, 00h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{short_label}:"));
                self.line("    ld a, 01h");
            }
            _ => unreachable!("not a logical op"),
        }
        self.line(&format!("{end_label}:"));
        Ok(())
    }

    fn emit_compare_hl_de(&mut self) {
        if is_intel_8080_family(self.cpu) {
            let low_byte = self.next_label("compare_low_byte");
            let end = self.next_label("compare_end");
            self.line("    ld a, h");
            self.line("    cp d");
            self.line(&format!("    jp z, {low_byte}"));
            self.line(&format!("    jp {end}"));
            self.line(&format!("{low_byte}:"));
            self.line("    ld a, l");
            self.line("    cp e");
            self.line(&format!("{end}:"));
        } else {
            self.line("    or a");
            self.line("    sbc hl, de");
        }
    }

    fn emit_wide_comparison(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        self.emit_expr_to_hl(left, width)?;
        self.line("    push hl");
        self.emit_expr_to_hl(right, width)?;
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.emit_compare_hl_de();
        self.emit_comparison_from_flags(op);
        Ok(())
    }

    fn emit_signed_comparison(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            if width == ValueWidth::U8 {
                let left_var = self.alloc_var(width.bytes());
                self.emit_expr_to_width(left, width)?;
                self.emit_store_width(left_var);
                self.emit_expr_to_width(right, width)?;
                self.line("    ld c, a");
                self.emit_load_width(left_var);
                self.emit_comparison(op);
                return Ok(());
            }
            return self.emit_wide_comparison(left, op, right, width);
        }

        let left_var = self.alloc_var(width.bytes());
        let right_var = self.alloc_var(width.bytes());
        let same_sign_label = self.next_label("scmp_same_sign");
        let true_label = self.next_label("scmp_true");
        let false_label = self.next_label("scmp_false");
        let end_label = self.next_label("scmp_end");
        let sign_offset = u32::from(width.bytes() - 1);

        self.emit_expr_to_width(left, width)?;
        self.emit_store_width(left_var);
        self.emit_expr_to_width(right, width)?;
        self.emit_store_width(right_var);

        self.line("    ld a, 80h");
        self.line("    ld c, a");
        self.line(&format!("    ld a, ({:06X}h)", left_var.addr + sign_offset));
        self.line("    and c");
        self.line("    ld b, a");
        self.line(&format!(
            "    ld a, ({:06X}h)",
            right_var.addr + sign_offset
        ));
        self.line("    and c");
        self.line("    cp b");
        self.line(&format!("    jp z, {same_sign_label}"));
        self.line("    ld a, b");
        self.line("    or a");
        match op {
            BinaryOp::Lt | BinaryOp::Le => {
                self.line(&format!("    jp nz, {true_label}"));
                self.line(&format!("    jp {false_label}"));
            }
            BinaryOp::Gt | BinaryOp::Ge => {
                self.line(&format!("    jp nz, {false_label}"));
                self.line(&format!("    jp {true_label}"));
            }
            _ => unreachable!("not a signed ordering comparison"),
        }

        self.line(&format!("{same_sign_label}:"));
        if width == ValueWidth::U8 {
            self.emit_load_width(right_var);
            self.line("    ld c, a");
            self.emit_load_width(left_var);
            self.line("    cp c");
        } else {
            self.emit_load_width(left_var);
            self.line("    push hl");
            self.emit_load_width(right_var);
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.emit_compare_hl_de();
        }
        match op {
            BinaryOp::Lt => self.line(&format!("    jp c, {true_label}")),
            BinaryOp::Ge => self.line(&format!("    jp nc, {true_label}")),
            BinaryOp::Le => {
                self.line(&format!("    jp c, {true_label}"));
                self.line(&format!("    jp z, {true_label}"));
            }
            BinaryOp::Gt => {
                self.line(&format!("    jp c, {false_label}"));
                self.line(&format!("    jp z, {false_label}"));
                self.line(&format!("    jp {true_label}"));
            }
            _ => unreachable!("not a signed ordering comparison"),
        }

        self.line(&format!("{false_label}:"));
        self.line("    ld a, 00h");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{true_label}:"));
        self.line("    ld a, 01h");
        self.line(&format!("{end_label}:"));
        Ok(())
    }

    fn emit_u8_div_mod(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: BinaryOp,
    ) -> Result<(), Diagnostic> {
        let left_var = self.alloc_var(1u32);
        self.emit_expr_to_a(left)?;
        self.emit_store_a(left_var);
        self.emit_expr_to_a(right)?;
        self.line("    ld c, a");
        self.emit_load_a(left_var);
        match op {
            BinaryOp::Div => self.line("    call __ezra_div_u8"),
            BinaryOp::Mod => self.line("    call __ezra_mod_u8"),
            _ => unreachable!("not a division op"),
        }
        Ok(())
    }

    fn constant_integer_value(&self, expr: &Expr) -> Result<Option<i64>, Diagnostic> {
        let is_constant = match expr {
            Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Char(_) | Expr::Bool(_) => true,
            Expr::Ident(name) => {
                self.local_constant(name).is_some() || self.symbols.constants.contains_key(name)
            }
            Expr::Unary { expr, .. } => self.constant_integer_value(expr)?.is_some(),
            Expr::Binary { left, right, .. } => {
                self.constant_integer_value(left)?.is_some()
                    && self.constant_integer_value(right)?.is_some()
            }
            Expr::Cast { expr, .. } => self.constant_integer_value(expr)?.is_some(),
            _ => false,
        };
        if is_constant {
            Ok(Some(self.eval_i64_with_local_constants(expr)?))
        } else {
            Ok(None)
        }
    }

    fn constant_mul_choice(
        &self,
        width: ValueWidth,
        factor: i64,
    ) -> Option<(u32, ConstantMulForm)> {
        let (raw, magnitude, negate) = constant_mul_factor(factor, width)?;
        let model = constant_mul_cost_model(self.cpu);
        let binary_digits = binary_constant_mul_digits(magnitude);
        let naf_digits = naf_constant_mul_digits(magnitude);
        let mut forms = vec![
            ConstantMulForm::ShiftAdd {
                digits: binary_digits.clone(),
                negate,
            },
            ConstantMulForm::ShiftAdd {
                digits: naf_digits.clone(),
                negate,
            },
        ];
        let mut candidates = vec![
            CostCandidate::new(
                "shift-add-binary",
                constant_mul_shift_add_cost(width, &binary_digits, negate),
            ),
            CostCandidate::new(
                "shift-add-signed",
                constant_mul_shift_add_cost(width, &naf_digits, negate),
            ),
        ];
        if matches!(self.cpu, CpuFamily::Ez80 | CpuFamily::R800) && width == ValueWidth::U8 {
            forms.push(ConstantMulForm::NativeU8);
            let native_cost = if self.cpu == CpuFamily::R800 {
                // LD C,n + MULUB A,C + LD A,L. MULUB itself takes 14 R800 clocks.
                InstructionCost::new(5, 16, 0, FlagEffects::writes(FlagSet::ALL))
            } else {
                InstructionCost::new(6, 23, 0, FlagEffects::writes(FlagSet::ALL))
            };
            candidates.push(CostCandidate::new("native", native_cost));
        }
        forms.push(ConstantMulForm::Helper);
        candidates.push(CostCandidate::new(
            "helper",
            constant_mul_helper_cost(self.cpu, width, raw),
        ));
        model
            .choose_index(&candidates)
            .map(|index| (raw, forms[index].clone()))
    }

    fn emit_constant_mul_from_loaded(
        &mut self,
        width: ValueWidth,
        raw: u32,
        signed: bool,
        form: &ConstantMulForm,
    ) {
        match form {
            ConstantMulForm::ShiftAdd { digits, negate } => {
                self.emit_constant_shift_add(width, digits, *negate);
            }
            ConstantMulForm::NativeU8 => {
                debug_assert_eq!(width, ValueWidth::U8);
                if self.cpu == CpuFamily::R800 {
                    self.line(&format!("    ld c, {:02X}h", raw & 0xFF));
                    self.line("    mulub a, c");
                    self.line("    ld a, l");
                } else {
                    self.line("    ld b, a");
                    self.line(&format!("    ld c, {:02X}h", raw & 0xFF));
                    self.line("    mlt bc");
                    self.line("    ld a, c");
                }
            }
            ConstantMulForm::Helper => match width {
                ValueWidth::U8 => {
                    self.line(&format!("    ld c, {:02X}h", raw & 0xFF));
                    self.line("    call __ezra_mul_u8");
                }
                ValueWidth::U16 => {
                    self.line(&format!("    ld bc, {:06X}h", raw & 0xFFFF));
                    self.line("    call __ezra_mul_u16");
                }
                ValueWidth::U24 => {
                    self.line(&format!("    ld bc, {:06X}h", raw & 0xFF_FFFF));
                    if signed {
                        self.line("    call __ezra_mul_i24");
                    } else {
                        self.line("    call __ezra_mul_u24");
                    }
                }
            },
        }
    }

    fn emit_constant_shift_add(&mut self, width: ValueWidth, digits: &[i8], negate: bool) {
        match width {
            ValueWidth::U8 => {
                self.line("    ld b, a");
                for digit in digits {
                    self.line("    add a, a");
                    match digit {
                        1 => self.line("    add a, b"),
                        -1 => self.line("    sub b"),
                        0 => {}
                        _ => unreachable!("constant multiplication digit is not -1, 0, or 1"),
                    }
                }
                if negate {
                    self.line("    ld b, a");
                    self.line("    xor a");
                    self.line("    sub b");
                }
            }
            ValueWidth::U16 | ValueWidth::U24 => {
                self.line("    push hl");
                self.line("    pop de");
                for digit in digits {
                    self.line("    add hl, hl");
                    match digit {
                        1 => self.line("    add hl, de"),
                        -1 => {
                            self.line("    or a");
                            self.line("    sbc hl, de");
                        }
                        0 => {}
                        _ => unreachable!("constant multiplication digit is not -1, 0, or 1"),
                    }
                }
                if negate {
                    self.line("    push hl");
                    self.line("    ld hl, 000000h");
                    self.line("    pop bc");
                    self.line("    or a");
                    self.line("    sbc hl, bc");
                }
                if width == ValueWidth::U16 {
                    self.zero_extend_hl16();
                }
            }
        }
    }

    fn emit_mul_to_width(
        &mut self,
        left: &Expr,
        right: &Expr,
        width: ValueWidth,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if let Some(factor) = self.constant_integer_value(right)?
            && let Some((raw, form)) = self.constant_mul_choice(width, factor)
        {
            self.emit_expr_to_width(left, width)?;
            self.emit_constant_mul_from_loaded(width, raw, signed, &form);
            return Ok(());
        }
        if width == ValueWidth::U8 {
            let left_var = self.alloc_var(1u32);
            self.emit_expr_to_a(left)?;
            self.emit_store_a(left_var);
            self.emit_expr_to_a(right)?;
            self.line("    ld c, a");
            self.emit_load_a(left_var);
            self.line("    call __ezra_mul_u8");
            return Ok(());
        }
        if width == ValueWidth::U16 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            self.line("    call __ezra_mul_u16");
            return Ok(());
        }
        if width == ValueWidth::U24 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            if signed {
                self.line("    call __ezra_mul_i24");
            } else {
                self.line("    call __ezra_mul_u24");
            }
            return Ok(());
        }

        let left_var = self.alloc_var(width.bytes());
        let counter = self.alloc_var(width.bytes());
        let result = self.alloc_var(width.bytes());
        let loop_label = self.next_label("mul_loop");
        let done_label = self.next_label("mul_done");

        self.emit_expr_to_width(left, width)?;
        self.emit_store_width(left_var);
        self.emit_expr_to_width(right, width)?;
        self.emit_store_width(counter);
        match width {
            ValueWidth::U8 => self.line("    xor a"),
            ValueWidth::U16 | ValueWidth::U24 => self.line("    ld hl, 000000h"),
        }
        self.emit_store_width(result);

        self.line(&format!("{loop_label}:"));
        self.emit_jump_if_memory_zero(counter, &done_label);
        if width == ValueWidth::U8 {
            self.emit_load_a(result);
            self.line("    ld b, a");
            self.emit_load_a(left_var);
            self.line("    add a, b");
            self.emit_store_a(result);
        } else {
            self.emit_load_width(result);
            self.line("    push hl");
            self.emit_load_width(left_var);
            self.line("    pop bc");
            self.emit_wide_op_with_left_in_bc(BinaryOp::Add, width)?;
            self.emit_store_width(result);
        }
        self.emit_decrement_memory(counter);
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done_label}:"));
        self.emit_load_width(result);
        Ok(())
    }

    fn emit_div_mod_to_width(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U16 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_u16"),
                BinaryOp::Mod => self.line("    call __ezra_mod_u16"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }
        if width == ValueWidth::U24 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_u24"),
                BinaryOp::Mod => self.line("    call __ezra_mod_u24"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }

        let dividend = self.alloc_var(width.bytes());
        let divisor = self.alloc_var(width.bytes());
        let quotient = self.alloc_var(width.bytes());
        let loop_label = self.next_label("div_loop");
        let zero_label = self.next_label("div_zero");
        let done_label = self.next_label("div_done");

        self.emit_expr_to_hl(left, width)?;
        self.emit_store_width(dividend);
        self.emit_expr_to_hl(right, width)?;
        self.emit_store_width(divisor);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_zero_variable(quotient);

        self.line(&format!("{loop_label}:"));
        self.emit_load_width(dividend);
        self.line("    push hl");
        self.emit_load_width(divisor);
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.line("    or a");
        self.line("    sbc hl, de");
        self.line(&format!("    jp c, {done_label}"));
        self.emit_store_width(dividend);
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        Ok(())
    }

    fn emit_signed_div_mod_to_width(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: BinaryOp,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U24 {
            self.emit_expr_to_hl(left, width)?;
            self.line("    push hl");
            self.emit_expr_to_hl(right, width)?;
            self.line("    push hl");
            self.line("    pop bc");
            self.line("    pop hl");
            match op {
                BinaryOp::Div => self.line("    call __ezra_div_i24"),
                BinaryOp::Mod => self.line("    call __ezra_mod_i24"),
                _ => unreachable!("not a division op"),
            }
            return Ok(());
        }

        let dividend = self.alloc_var(width.bytes());
        let divisor = self.alloc_var(width.bytes());
        let quotient = self.alloc_var(width.bytes());
        let quotient_negative = self.alloc_var(ValueWidth::U8.bytes());
        let remainder_negative = self.alloc_var(ValueWidth::U8.bytes());
        let loop_label = self.next_label("sdiv_loop");
        let zero_label = self.next_label("sdiv_zero");
        let done_label = self.next_label("sdiv_done");
        let quotient_positive_label = self.next_label("sdiv_q_positive");
        let remainder_positive_label = self.next_label("sdiv_r_positive");
        let not_overflow_label = self.next_label("sdiv_not_overflow");
        let finished_label = self.next_label("sdiv_finished");

        self.emit_expr_to_width(left, width)?;
        self.emit_store_width(dividend);
        self.emit_expr_to_width(right, width)?;
        self.emit_store_width(divisor);
        self.emit_zero_variable(quotient);
        self.emit_zero_variable(quotient_negative);
        self.emit_zero_variable(remainder_negative);
        self.emit_jump_if_memory_zero(divisor, &zero_label);
        self.emit_jump_if_memory_not_equals(dividend, signed_min_bytes(width), &not_overflow_label);
        self.emit_jump_if_memory_not_equals(
            divisor,
            signed_negative_one_bytes(width),
            &not_overflow_label,
        );
        match op {
            BinaryOp::Div => self.emit_load_width(dividend),
            BinaryOp::Mod => {
                self.emit_zero_variable(dividend);
                self.emit_load_width(dividend);
            }
            _ => unreachable!("not a division op"),
        }
        self.line(&format!("    jp {finished_label}"));
        self.line(&format!("{not_overflow_label}:"));

        self.emit_abs_signed_variable(dividend, Some(quotient_negative), Some(remainder_negative));
        self.emit_abs_signed_variable(divisor, Some(quotient_negative), None);

        self.line(&format!("{loop_label}:"));
        if width == ValueWidth::U8 {
            self.emit_load_a(dividend);
            self.line("    ld b, a");
            self.emit_load_a(divisor);
            self.line("    ld c, a");
            self.line("    ld a, b");
            self.line("    cp c");
            self.line(&format!("    jp c, {done_label}"));
            self.line("    sub c");
            self.emit_store_a(dividend);
        } else {
            self.emit_load_width(dividend);
            self.line("    push hl");
            self.emit_load_width(divisor);
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.line("    or a");
            self.line("    sbc hl, de");
            self.line(&format!("    jp c, {done_label}"));
            self.emit_store_width(dividend);
        }
        self.emit_increment_memory(quotient);
        self.line(&format!("    jp {loop_label}"));

        self.line(&format!("{zero_label}:"));
        self.emit_zero_variable(dividend);
        self.emit_zero_variable(quotient);
        self.line(&format!("{done_label}:"));
        self.emit_load_a(quotient_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {quotient_positive_label}"));
        self.emit_negate_memory(quotient);
        self.line(&format!("{quotient_positive_label}:"));
        self.emit_load_a(remainder_negative);
        self.line("    or a");
        self.line(&format!("    jp z, {remainder_positive_label}"));
        self.emit_negate_memory(dividend);
        self.line(&format!("{remainder_positive_label}:"));

        match op {
            BinaryOp::Div => self.emit_load_width(quotient),
            BinaryOp::Mod => self.emit_load_width(dividend),
            _ => unreachable!("not a division op"),
        }
        self.line(&format!("{finished_label}:"));
        Ok(())
    }

    fn emit_abs_signed_variable(
        &mut self,
        variable: Variable,
        quotient_negative: Option<Variable>,
        remainder_negative: Option<Variable>,
    ) {
        let nonnegative_label = self.next_label("signed_nonnegative");
        let sign_addr = variable.addr + variable.size - 1;
        self.line(&format!("    ld a, ({sign_addr:06X}h)"));
        self.line("    ld b, a");
        self.line("    ld a, 7Fh");
        self.line("    cp b");
        self.line(&format!("    jp nc, {nonnegative_label}"));
        self.emit_negate_memory(variable);
        if let Some(flag) = quotient_negative {
            self.emit_toggle_u8(flag);
        }
        if let Some(flag) = remainder_negative {
            self.emit_toggle_u8(flag);
        }
        self.line(&format!("{nonnegative_label}:"));
    }

    fn emit_negate_memory(&mut self, variable: Variable) {
        for offset in 0..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    xor FFh");
            self.line(&format!("    ld ({addr:06X}h), a"));
        }
        self.emit_increment_memory(variable);
    }

    fn emit_toggle_u8(&mut self, variable: Variable) {
        self.emit_load_a(variable);
        self.line("    xor 01h");
        self.emit_store_a(variable);
    }

    fn emit_jump_if_memory_zero(&mut self, variable: Variable, zero_label: &str) {
        let nonzero_label = self.next_label("nonzero");
        for offset in 0..variable.size {
            self.line(&format!("    ld a, ({:06X}h)", variable.addr + offset));
            self.line("    or a");
            self.line(&format!("    jp nz, {nonzero_label}"));
        }
        self.line(&format!("    jp {zero_label}"));
        self.line(&format!("{nonzero_label}:"));
    }

    fn emit_jump_if_memory_not_equals(&mut self, variable: Variable, bytes: &[u8], label: &str) {
        for (offset, byte) in bytes.iter().copied().enumerate() {
            self.line(&format!(
                "    ld a, ({:06X}h)",
                variable.addr + offset as u32
            ));
            self.line("    ld b, a");
            self.line(&format!("    ld a, {byte:02X}h"));
            self.line("    cp b");
            self.line(&format!("    jp nz, {label}"));
        }
    }

    fn emit_zero_variable(&mut self, variable: Variable) {
        match variable.size {
            1 => self.line("    xor a"),
            2 | 3 => self.line("    ld hl, 000000h"),
            _ => unreachable!("unsupported variable size {}", variable.size),
        }
        self.emit_store_width(variable);
    }

    fn emit_zero_storage(&mut self, variable: Variable) {
        self.line("    xor a");
        for offset in 0..variable.size {
            self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
        }
    }

    fn emit_increment_memory(&mut self, variable: Variable) {
        let done_label = self.next_label("inc_done");
        for offset in 0..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    ld b, a");
            self.line("    ld a, 01h");
            self.line("    add a, b");
            self.line(&format!("    ld ({addr:06X}h), a"));
            self.line("    or a");
            self.line(&format!("    jp nz, {done_label}"));
        }
        self.line(&format!("{done_label}:"));
    }

    fn emit_decrement_memory(&mut self, variable: Variable) {
        let done_label = self.next_label("dec_done");
        for offset in 0..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    ld b, a");
            self.line("    ld a, 01h");
            self.line("    ld c, a");
            self.line("    ld a, b");
            self.line("    sub c");
            self.line(&format!("    ld ({addr:06X}h), a"));
            self.line("    ld a, b");
            self.line("    or a");
            self.line(&format!("    jp nz, {done_label}"));
        }
        self.line(&format!("{done_label}:"));
    }

    fn emit_shift_a(&mut self, op: BinaryOp, count: u8, signed: bool) -> Result<(), Diagnostic> {
        for _ in 0..count {
            match op {
                BinaryOp::Shl => self.line("    add a, a"),
                BinaryOp::Shr if signed => self.line("    sra a"),
                BinaryOp::Shr => self.line("    srl a"),
                _ => unreachable!("not a shift op"),
            }
        }
        Ok(())
    }

    fn emit_shift_a_by_expr(
        &mut self,
        op: BinaryOp,
        count: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if let Some(count) = self.maybe_const_shift_count(count)? {
            return self.emit_shift_a(op, count, signed);
        }
        let temp = self.alloc_var(ValueWidth::U8.bytes());
        self.emit_store_a(temp);
        self.emit_expr_to_a(count)?;
        self.line("    ld b, a");
        self.emit_shift_memory_dynamic(temp, op, signed)?;
        self.emit_load_a(temp);
        Ok(())
    }

    fn emit_shift_memory(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        count: u8,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        for _ in 0..count {
            match op {
                BinaryOp::Shl => self.emit_shift_memory_left_once(variable),
                BinaryOp::Shr => self.emit_shift_memory_right_once(variable, signed),
                _ => unreachable!("not a shift op"),
            }
        }
        Ok(())
    }

    fn emit_shift_memory_by_expr(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        count: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if let Some(count) = self.maybe_const_shift_count(count)? {
            return self.emit_shift_memory(variable, op, count, signed);
        }
        self.emit_expr_to_a(count)?;
        self.line("    ld b, a");
        self.emit_shift_memory_dynamic(variable, op, signed)
    }

    fn emit_shift_temporary_by_expr(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        count: &Expr,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if let Some(count) = self.maybe_const_shift_count(count)?
            && count != 0
            && count % 8 == 0
        {
            self.emit_byte_aligned_shift_temporary(variable, op, count / 8, signed);
            return Ok(());
        }
        self.emit_shift_memory_by_expr(variable, op, count, signed)
    }

    fn emit_byte_aligned_shift_temporary(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        byte_count: u8,
        signed: bool,
    ) {
        let size = variable.size as u8;
        let byte_count = byte_count.min(size);

        match op {
            BinaryOp::Shl => {
                for offset in (byte_count..size).rev() {
                    let source = variable.addr + u32::from(offset - byte_count);
                    let destination = variable.addr + u32::from(offset);
                    self.line(&format!("    ld a, ({source:06X}h)"));
                    self.line(&format!("    ld ({destination:06X}h), a"));
                }
                self.emit_zero_shifted_bytes(variable, 0, byte_count);
            }
            BinaryOp::Shr => {
                for offset in 0..size.saturating_sub(byte_count) {
                    let source = variable.addr + u32::from(offset + byte_count);
                    let destination = variable.addr + u32::from(offset);
                    self.line(&format!("    ld a, ({source:06X}h)"));
                    self.line(&format!("    ld ({destination:06X}h), a"));
                }
                if signed {
                    self.line(&format!(
                        "    ld a, ({:06X}h)",
                        variable.addr + u32::from(size - 1)
                    ));
                    self.line("    add a, a");
                    self.line("    sbc a, a");
                    for offset in size.saturating_sub(byte_count)..size {
                        let destination = variable.addr + u32::from(offset);
                        self.line(&format!("    ld ({destination:06X}h), a"));
                    }
                } else {
                    self.emit_zero_shifted_bytes(
                        variable,
                        size.saturating_sub(byte_count),
                        byte_count,
                    );
                }
            }
            _ => unreachable!("not a shift op"),
        }
    }

    fn emit_zero_shifted_bytes(&mut self, variable: Variable, start: u8, count: u8) {
        if count == 0 {
            return;
        }
        self.line("    xor a");
        for offset in start..start + count {
            let destination = variable.addr + u32::from(offset);
            self.line(&format!("    ld ({destination:06X}h), a"));
        }
    }

    fn emit_shift_memory_dynamic(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        if !supports_z80_bit_instructions(self.cpu) {
            return self.emit_shift_memory_dynamic_bitwise(variable, op, signed);
        }

        let saturated_label = self.next_label("shift_saturated");
        let byte_loop_label = self.next_label("shift_byte_loop");
        let byte_done_label = self.next_label("shift_byte_done");
        let bit_loop_label = self.next_label("shift_bit_loop");
        let done_label = self.next_label("shift_done");
        let after_saturated_label = self.next_label("shift_after_saturated");
        let bit_width = variable.size * 8;

        self.line("    ld a, b");
        self.line(&format!("    cp {bit_width:02X}h"));
        self.line(&format!("    jp nc, {saturated_label}"));

        if variable.size > 1 {
            self.line("    ld a, b");
            self.line("    and 07h");
            self.line("    ld d, a");
            self.line("    ld a, b");
            for _ in 0..3 {
                self.line("    srl a");
            }
            self.line("    ld b, a");
            self.line(&format!("{byte_loop_label}:"));
            self.line("    ld a, b");
            self.line("    or a");
            self.line(&format!("    jp z, {byte_done_label}"));
            match op {
                BinaryOp::Shl => self.emit_shift_memory_left_byte(variable),
                BinaryOp::Shr => self.emit_shift_memory_right_byte(variable, signed),
                _ => unreachable!("not a shift op"),
            }
            self.line("    dec b");
            self.line(&format!("    jp {byte_loop_label}"));
            self.line(&format!("{byte_done_label}:"));
            self.line("    ld b, d");
        } else {
            self.line("    ld a, b");
            self.line("    and 07h");
            self.line("    ld b, a");
        }

        self.line(&format!("{bit_loop_label}:"));
        self.line("    ld a, b");
        self.line("    or a");
        self.line(&format!("    jp z, {done_label}"));
        match op {
            BinaryOp::Shl => self.emit_shift_memory_left_once(variable),
            BinaryOp::Shr => self.emit_shift_memory_right_once(variable, signed),
            _ => unreachable!("not a shift op"),
        }
        self.line("    dec b");
        self.line(&format!("    jp {bit_loop_label}"));
        self.line(&format!("{done_label}:"));
        self.line(&format!("    jp {after_saturated_label}"));

        self.line(&format!("{saturated_label}:"));
        self.emit_saturated_shift_memory(variable, op, signed);
        self.line(&format!("{after_saturated_label}:"));
        Ok(())
    }

    fn emit_shift_memory_dynamic_bitwise(
        &mut self,
        variable: Variable,
        op: BinaryOp,
        signed: bool,
    ) -> Result<(), Diagnostic> {
        let loop_label = self.next_label("shift_loop");
        let done_label = self.next_label("shift_done");
        self.line(&format!("{loop_label}:"));
        self.line("    ld a, b");
        self.line("    or a");
        self.line(&format!("    jp z, {done_label}"));
        match op {
            BinaryOp::Shl => self.emit_shift_memory_left_once(variable),
            BinaryOp::Shr => self.emit_shift_memory_right_once(variable, signed),
            _ => unreachable!("not a shift op"),
        }
        self.line("    dec b");
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done_label}:"));
        Ok(())
    }

    fn emit_shift_memory_left_byte(&mut self, variable: Variable) {
        for offset in (1..variable.size).rev() {
            let source = variable.addr + offset - 1;
            let destination = variable.addr + offset;
            self.line(&format!("    ld a, ({source:06X}h)"));
            self.line(&format!("    ld ({destination:06X}h), a"));
        }
        self.line("    xor a");
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
    }

    fn emit_shift_memory_right_byte(&mut self, variable: Variable, signed: bool) {
        for offset in 0..variable.size.saturating_sub(1) {
            let source = variable.addr + offset + 1;
            let destination = variable.addr + offset;
            self.line(&format!("    ld a, ({source:06X}h)"));
            self.line(&format!("    ld ({destination:06X}h), a"));
        }
        if signed {
            self.line(&format!(
                "    ld a, ({:06X}h)",
                variable.addr + variable.size - 1
            ));
            self.line("    add a, a");
            self.line("    sbc a, a");
        } else {
            self.line("    xor a");
        }
        self.line(&format!(
            "    ld ({:06X}h), a",
            variable.addr + variable.size - 1
        ));
    }

    fn emit_saturated_shift_memory(&mut self, variable: Variable, op: BinaryOp, signed: bool) {
        match op {
            BinaryOp::Shl => self.line("    xor a"),
            BinaryOp::Shr if signed => {
                self.line(&format!(
                    "    ld a, ({:06X}h)",
                    variable.addr + variable.size - 1
                ));
                self.line("    add a, a");
                self.line("    sbc a, a");
            }
            BinaryOp::Shr => self.line("    xor a"),
            _ => unreachable!("not a shift op"),
        }
        for offset in 0..variable.size {
            self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
        }
    }

    fn emit_shift_memory_left_once(&mut self, variable: Variable) {
        self.line(&format!("    ld a, ({:06X}h)", variable.addr));
        self.line("    add a, a");
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
        for offset in 1..variable.size {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            self.line("    rla");
            self.line(&format!("    ld ({addr:06X}h), a"));
        }
    }

    fn emit_shift_memory_right_once(&mut self, variable: Variable, signed: bool) {
        for offset in (0..variable.size).rev() {
            let addr = variable.addr + offset;
            self.line(&format!("    ld a, ({addr:06X}h)"));
            if offset == variable.size - 1 {
                if signed {
                    self.line("    sra a");
                } else {
                    self.line("    srl a");
                }
            } else {
                self.line("    rra");
            }
            self.line(&format!("    ld ({addr:06X}h), a"));
        }
    }

    fn emit_unary_to_a(&mut self, op: UnaryOp, expr: &Expr) -> Result<(), Diagnostic> {
        match op {
            UnaryOp::Neg => {
                self.emit_expr_to_a(expr)?;
                self.line("    ld b, a");
                self.line("    xor a");
                self.line("    sub b");
            }
            UnaryOp::BitNot => {
                self.emit_expr_to_a(expr)?;
                self.line("    xor FFh");
            }
            UnaryOp::Not => {
                let true_label = self.next_label("not_true");
                let end_label = self.next_label("not_end");
                self.emit_expr_to_a(expr)?;
                self.line("    or a");
                self.line(&format!("    jp z, {true_label}"));
                self.line("    ld a, 00h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{true_label}:"));
                self.line("    ld a, 01h");
                self.line(&format!("{end_label}:"));
            }
        }
        Ok(())
    }

    fn emit_unary_to_hl(
        &mut self,
        op: UnaryOp,
        expr: &Expr,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        match op {
            UnaryOp::Neg => {
                self.emit_expr_to_hl(expr, width)?;
                self.line("    push hl");
                self.line("    ld hl, 000000h");
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
            }
            UnaryOp::BitNot => {
                self.emit_expr_to_hl(expr, width)?;
                let value = self.alloc_var(width.bytes());
                self.emit_store_width(value);
                let result = self.alloc_var(width.bytes());
                for offset in 0..width.bytes() {
                    self.line(&format!("    ld a, ({:06X}h)", value.addr + offset as u32));
                    self.line("    xor FFh");
                    self.line(&format!("    ld ({:06X}h), a", result.addr + offset as u32));
                }
                self.emit_load_width(result);
            }
            UnaryOp::Not => {
                let true_label = self.next_label("not_true");
                let end_label = self.next_label("not_end");
                self.emit_expr_to_hl(expr, width)?;
                self.line("    push hl");
                self.line("    ld hl, 000000h");
                self.line("    pop bc");
                self.line("    or a");
                self.line("    sbc hl, bc");
                self.line(&format!("    jp z, {true_label}"));
                self.line("    ld hl, 000000h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{true_label}:"));
                self.line("    ld hl, 000001h");
                self.line(&format!("{end_label}:"));
            }
        }
        Ok(())
    }

    fn emit_comparison(&mut self, op: BinaryOp) {
        self.line("    cp c");
        self.emit_comparison_from_flags(op);
    }

    fn emit_u16_immediate_bitwise(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        width: ValueWidth,
    ) -> Result<bool, Diagnostic> {
        if width != ValueWidth::U16
            || !matches!(op, BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor)
        {
            return Ok(false);
        }
        self.validate_typed_literal_ranges(right)?;
        let Ok(mask) = self.eval_i64_with_local_constants(right) else {
            return Ok(false);
        };

        self.emit_expr_to_hl(left, width)?;
        self.emit_u8_immediate_bitwise_register("l", op, mask as u8);
        self.emit_u8_immediate_bitwise_register("h", op, (mask >> 8) as u8);
        Ok(true)
    }

    fn emit_u8_immediate_bitwise_register(&mut self, register: &str, op: BinaryOp, mask: u8) {
        let is_identity = matches!(
            (op, mask),
            (BinaryOp::BitAnd, 0xFF) | (BinaryOp::BitOr, 0x00) | (BinaryOp::BitXor, 0x00)
        );
        if is_identity {
            return;
        }

        match (op, mask) {
            (BinaryOp::BitAnd, 0x00) => self.line(&format!("    ld {register}, 00h")),
            (BinaryOp::BitOr, 0xFF) => self.line(&format!("    ld {register}, FFh")),
            _ => {
                self.line(&format!("    ld a, {register}"));
                match op {
                    BinaryOp::BitAnd => self.line(&format!("    and {mask:02X}h")),
                    BinaryOp::BitOr => self.line(&format!("    or {mask:02X}h")),
                    BinaryOp::BitXor => self.line(&format!("    xor {mask:02X}h")),
                    _ => unreachable!("not a bitwise operation"),
                }
                self.line(&format!("    ld {register}, a"));
            }
        }
    }

    fn emit_single_bit_mask(
        &mut self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
        width: ValueWidth,
    ) -> Result<bool, Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(left)?)?;
        if !matches!(ty, Type::Named(ref name) if matches!(name.as_str(), "u8" | "u16" | "u24")) {
            return Ok(false);
        }
        let Ok(mask) = self.eval_i64_with_local_constants(right) else {
            return Ok(false);
        };
        let width_mask = (1_i64 << (width.bytes() * 8)) - 1;
        let mask = mask & width_mask;
        let bit_mask = match op {
            BinaryOp::BitOr | BinaryOp::BitXor if mask.count_ones() == 1 => mask,
            BinaryOp::BitAnd if (!mask & width_mask).count_ones() == 1 => !mask & width_mask,
            _ => return Ok(false),
        };
        let bit = bit_mask.trailing_zeros() as u8;
        let byte_offset = u32::from(bit / 8);
        let byte_mask = 1_u8 << (bit % 8);

        if op != BinaryOp::BitXor && !supports_z80_bit_instructions(self.cpu) {
            return Ok(false);
        }

        if width == ValueWidth::U8 {
            self.emit_expr_to_a(left)?;
            match op {
                BinaryOp::BitAnd => self.line(&format!("    res {}, a", bit % 8)),
                BinaryOp::BitOr => self.line(&format!("    set {}, a", bit % 8)),
                BinaryOp::BitXor => self.line(&format!("    xor {byte_mask:02X}h")),
                _ => unreachable!("not a bitwise mask operation"),
            }
            return Ok(true);
        }

        let value = self.alloc_var(width.bytes());
        self.emit_expr_to_hl(left, width)?;
        self.emit_store_width(value);
        self.line(&format!("    ld a, ({:06X}h)", value.addr + byte_offset));
        match op {
            BinaryOp::BitAnd => self.line(&format!("    res {}, a", bit % 8)),
            BinaryOp::BitOr => self.line(&format!("    set {}, a", bit % 8)),
            BinaryOp::BitXor => self.line(&format!("    xor {byte_mask:02X}h")),
            _ => unreachable!("not a bitwise mask operation"),
        }
        self.line(&format!("    ld ({:06X}h), a", value.addr + byte_offset));
        self.emit_load_width(value);
        Ok(true)
    }

    fn emit_masked_byte_to_a(
        &mut self,
        source: &Expr,
        width: ValueWidth,
        byte_offset: u32,
    ) -> Result<(), Diagnostic> {
        if width == ValueWidth::U8 {
            return self.emit_expr_to_a(source);
        }

        let value = self.alloc_var(width.bytes());
        self.emit_expr_to_hl(source, width)?;
        self.emit_store_width(value);
        self.line(&format!("    ld a, ({:06X}h)", value.addr + byte_offset));
        Ok(())
    }

    /// Emits a false branch without materializing a boolean for byte equality
    /// and inequality against an immediate. Returns whether it handled `condition`.
    fn emit_jump_if_false(
        &mut self,
        condition: &Expr,
        false_label: &str,
    ) -> Result<bool, Diagnostic> {
        let Expr::Binary { left, op, right } = condition else {
            return Ok(false);
        };
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne)
            && let Expr::Binary {
                left: masked,
                op: BinaryOp::BitAnd,
                right: mask_expr,
            } = left.as_ref()
            && let Ok(raw_mask) = self.eval_i64_with_local_constants(mask_expr)
            && let Ok(raw_expected) = self.eval_i64_with_local_constants(right)
        {
            let width = self.expr_width(masked)?;
            let width_mask = (1_i64 << (width.bytes() * 8)) - 1;
            let mask = raw_mask & width_mask;
            let expected = raw_expected & width_mask;
            let bit_offset = mask.trailing_zeros();
            let byte_offset = bit_offset / 8;
            let byte_mask = mask >> (byte_offset * 8);

            if mask != 0
                && mask == byte_mask << (byte_offset * 8)
                && (expected == 0 || expected == mask)
            {
                if supports_z80_bit_instructions(self.cpu) && (mask as u64).is_power_of_two() {
                    self.emit_masked_byte_to_a(masked, width, byte_offset)?;
                    self.line(&format!("    bit {}, a", bit_offset % 8));
                    let set_when_true = (*op == BinaryOp::Eq && expected == mask)
                        || (*op == BinaryOp::Ne && expected == 0);
                    let branch = if set_when_true { "z" } else { "nz" };
                    self.line(&format!("    jp {branch}, {false_label}"));
                    return Ok(true);
                }

                self.emit_masked_byte_to_a(masked, width, byte_offset)?;
                self.line(&format!("    and {byte_mask:02X}h"));
                if expected == mask {
                    self.line(&format!("    cp {byte_mask:02X}h"));
                }
                let branch = if *op == BinaryOp::Eq { "nz" } else { "z" };
                self.line(&format!("    jp {branch}, {false_label}"));
                return Ok(true);
            }
        }

        if matches!(op, BinaryOp::Eq | BinaryOp::Ne)
            && self.expr_width(left)? == ValueWidth::U8
            && self.expr_width(right)? == ValueWidth::U8
            && is_immediate_u8(right)
        {
            self.emit_expr_to_a(left)?;
            let immediate = self.u8(right)?;
            if immediate == 0 {
                self.line("    or a");
            } else {
                self.line(&format!("    cp {immediate:02X}h"));
            }
            let branch = if *op == BinaryOp::Eq { "nz" } else { "z" };
            self.line(&format!("    jp {branch}, {false_label}"));
            return Ok(true);
        }

        let width = self.expr_width(left)?.max(self.expr_width(right)?);
        if width == ValueWidth::U8 || self.binary_operands_are_signed(left, right)? {
            return Ok(false);
        }
        self.emit_expr_to_hl(left, width)?;
        self.line("    push hl");
        self.emit_expr_to_hl(right, width)?;
        self.line("    ex de, hl");
        self.line("    pop hl");
        self.emit_compare_hl_de();
        match op {
            BinaryOp::Eq => self.line(&format!("    jp nz, {false_label}")),
            BinaryOp::Ne => self.line(&format!("    jp z, {false_label}")),
            BinaryOp::Lt => self.line(&format!("    jp nc, {false_label}")),
            BinaryOp::Ge => self.line(&format!("    jp c, {false_label}")),
            BinaryOp::Gt => {
                self.line(&format!("    jp c, {false_label}"));
                self.line(&format!("    jp z, {false_label}"));
            }
            BinaryOp::Le => {
                let keep_going = self.next_label("cmp_le_true");
                self.line(&format!("    jp c, {keep_going}"));
                self.line(&format!("    jp nz, {false_label}"));
                self.line(&format!("{keep_going}:"));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn emit_comparison_from_flags(&mut self, op: BinaryOp) {
        let true_label = self.next_label("cmp_true");
        let end_label = self.next_label("cmp_end");
        let false_label = self.next_label("cmp_false");
        match op {
            BinaryOp::Eq => self.line(&format!("    jp z, {true_label}")),
            BinaryOp::Ne => self.line(&format!("    jp nz, {true_label}")),
            BinaryOp::Lt => self.line(&format!("    jp c, {true_label}")),
            BinaryOp::Ge => self.line(&format!("    jp nc, {true_label}")),
            BinaryOp::Le => {
                self.line(&format!("    jp c, {true_label}"));
                self.line(&format!("    jp z, {true_label}"));
            }
            BinaryOp::Gt => {
                self.line(&format!("    jp c, {false_label}"));
                self.line(&format!("    jp z, {false_label}"));
                self.line(&format!("    jp {true_label}"));
            }
            _ => unreachable!("not a comparison"),
        }
        self.line(&format!("{false_label}:"));
        self.line("    ld a, 00h");
        self.line(&format!("    jp {end_label}"));
        self.line(&format!("{true_label}:"));
        self.line("    ld a, 01h");
        self.line(&format!("{end_label}:"));
    }

    fn emit_out(&mut self, port: u8, value: u8) {
        self.line(&format!("    ld a, {:02X}h", value));
        self.emit_out_a(port);
    }

    fn emit_in_a(&mut self, port: u8) {
        if is_z80_family_16bit(self.cpu) {
            self.line(&format!("    in a, ({:02X}h)", port));
        } else {
            self.line(&format!("    in0 a, ({:02X}h)", port));
        }
    }

    fn emit_out_a(&mut self, port: u8) {
        if is_z80_family_16bit(self.cpu) {
            self.line(&format!("    out ({:02X}h), a", port));
        } else {
            self.line(&format!("    out0 ({:02X}h), a", port));
        }
    }

    fn emit_load_a(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 1);
        self.line(&format!("    ld a, ({:06X}h)", variable.addr));
    }

    fn emit_store_a(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 1);
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
    }

    fn emit_load_hl(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 3);
        self.line(&format!("    ld hl, ({:06X}h)", variable.addr));
    }

    fn emit_store_hl(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 3);
        self.line(&format!("    ld ({:06X}h), hl", variable.addr));
    }

    fn emit_load_hl16(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 2);
        self.line("    ld hl, 000000h");
        self.line(&format!("    ld a, ({:06X}h)", variable.addr));
        self.line("    ld l, a");
        self.line(&format!("    ld a, ({:06X}h)", variable.addr + 1));
        self.line("    ld h, a");
    }

    fn emit_store_hl16(&mut self, variable: Variable) {
        debug_assert_eq!(variable.size, 2);
        self.line("    ld a, l");
        self.line(&format!("    ld ({:06X}h), a", variable.addr));
        self.line("    ld a, h");
        self.line(&format!("    ld ({:06X}h), a", variable.addr + 1));
    }

    fn emit_load_width(&mut self, variable: Variable) {
        match variable.size {
            1 => self.emit_load_a(variable),
            2 => self.emit_load_hl16(variable),
            3 => self.emit_load_hl(variable),
            _ => unreachable!("unsupported variable size {}", variable.size),
        }
    }

    fn emit_store_width(&mut self, variable: Variable) {
        match variable.size {
            1 => self.emit_store_a(variable),
            2 => self.emit_store_hl16(variable),
            3 => self.emit_store_hl(variable),
            _ => unreachable!("unsupported variable size {}", variable.size),
        }
    }

    fn emit_load_ix_offset_width_into(
        &mut self,
        offset: u8,
        variable: Variable,
    ) -> Result<(), Diagnostic> {
        for byte_offset in 0..variable.size {
            let displacement = offset as u32 + byte_offset;
            if displacement > 0x7F {
                return Err(Diagnostic::new(format!(
                    "stack argument offset {displacement} exceeds frame displacement range"
                )));
            }
            if is_z80_family_16bit(self.cpu) {
                self.line(&format!("    ld hl, {displacement:04X}h"));
                self.line("    add hl, sp");
                self.line("    ld a, (hl)");
            } else {
                self.line(&format!("    ld a, (ix+{displacement})"));
            }
            self.line(&format!("    ld ({:06X}h), a", variable.addr + byte_offset));
        }
        Ok(())
    }

    fn emit_push_stack_arg_variable(&mut self, variable: Variable) {
        for byte_offset in (0..variable.size).rev() {
            self.line(&format!("    ld a, ({:06X}h)", variable.addr + byte_offset));
            self.line("    dec sp");
            self.line("    ld hl, 000000h");
            self.line("    add hl, sp");
            self.line("    ld (hl), a");
        }
    }

    fn emit_drop_stack_arg_bytes(&mut self, bytes: u8) {
        for _ in 0..bytes {
            self.line("    inc sp");
        }
    }

    fn emit_load_pointed_width_into(&mut self, variable: Variable) {
        for offset in 0..variable.size {
            if offset != 0 {
                self.line("    inc hl");
            }
            self.line("    ld a, (hl)");
            self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
        }
    }

    fn emit_store_var_to_pointed_width(&mut self, variable: Variable) {
        for offset in 0..variable.size {
            if offset != 0 {
                self.line("    inc hl");
            }
            self.line(&format!("    ld a, ({:06X}h)", variable.addr + offset));
            self.line("    ld (hl), a");
        }
    }

    fn emit_copy_storage_into(
        &mut self,
        source: Variable,
        target: Variable,
    ) -> Result<(), Diagnostic> {
        if source.size != target.size {
            return Err(Diagnostic::new("type mismatch"));
        }
        if storage_ranges_overlap(source, target) {
            let temp = self.alloc_var(source.size);
            self.emit_copy_storage_bytes(source, temp);
            self.emit_copy_storage_bytes(temp, target);
        } else {
            self.emit_copy_storage_bytes(source, target);
        }
        Ok(())
    }

    fn emit_copy_storage_bytes(&mut self, source: Variable, target: Variable) {
        for offset in 0..source.size {
            self.line(&format!("    ld a, ({:06X}h)", source.addr + offset));
            self.line(&format!("    ld ({:06X}h), a", target.addr + offset));
        }
    }

    fn emit_copy_pointed_storage_into(
        &mut self,
        ptr: &Expr,
        variable: Variable,
    ) -> Result<(), Diagnostic> {
        let temp = self.alloc_var(variable.size);
        self.emit_expr_to_hl(ptr, ValueWidth::U24)?;
        self.emit_load_pointed_width_into(temp);
        self.emit_copy_storage_bytes(temp, variable);
        Ok(())
    }

    fn expr_storage_variable(&self, expr: &Expr) -> Result<Option<Variable>, Diagnostic> {
        match expr {
            Expr::Ident(name) => Ok(self.variable_opt(name)),
            Expr::Field { base, field } => {
                if let Some(variable) = self.dotted_variable(base, field) {
                    return Ok(Some(variable));
                }
                if self.variable_opt(base).is_some() {
                    self.field_variable(base, field).map(Some)
                } else {
                    Ok(None)
                }
            }
            Expr::Index { name, index } => self.const_array_element_variable(name, index),
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty() {
                    return Ok(self.variable_opt(&path.root));
                }
                if self.variable_opt(&path.root).is_some() {
                    self.const_access_variable(&path)
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn const_array_element_variable(
        &self,
        name: &str,
        index: &Expr,
    ) -> Result<Option<Variable>, Diagnostic> {
        self.validate_array_index_type(index)?;
        if self.pointer_element_type(name)?.is_some() {
            return Ok(None);
        }
        let (array, element_size, len) = self.array_info(name)?;
        let index_value = match self.symbols.eval_i64(index) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        if index_value < 0 || index_value as u32 >= len {
            return Err(Diagnostic::new(format!(
                "array index {index_value} is out of bounds for `{name}` length {len}"
            )));
        }
        let element_type = self.array_element_type(name)?;
        self.symbols
            .storage_at(
                array.addr + index_value as u32 * element_size,
                &element_type,
            )
            .map(Some)
    }

    fn array_info(&self, name: &str) -> Result<(Variable, u32, u32), Diagnostic> {
        let array = self.variable(name)?;
        let element_size = array
            .element_size
            .ok_or_else(|| Diagnostic::new(format!("`{name}` is not an array")))?;
        let len = array
            .len
            .ok_or_else(|| Diagnostic::new(format!("array `{name}` is missing length")))?;
        Ok((array, element_size, len))
    }

    fn array_element_width(&self, name: &str) -> Result<ValueWidth, Diagnostic> {
        self.symbols.type_width(&self.array_element_type(name)?)
    }

    fn emit_variable_address(&mut self, name: &str) -> Result<(), Diagnostic> {
        let variable = self
            .symbols
            .embeds
            .get(name)
            .map(|embed| embed.variable)
            .or_else(|| self.variable_opt(name))
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{name}`")))?;
        self.line(&format!("    ld hl, {:06X}h", variable.addr));
        Ok(())
    }

    fn emit_field_address(&mut self, base: &str, field: &str) -> Result<(), Diagnostic> {
        let variable = self.field_variable(base, field)?;
        self.line(&format!("    ld hl, {:06X}h", variable.addr));
        Ok(())
    }

    fn struct_type_name(&self, ty: &Type) -> Result<String, Diagnostic> {
        match self.symbols.resolved_type(ty)? {
            Type::Named(name) if self.symbols.structs.contains_key(&name) => Ok(name),
            other => Err(Diagnostic::new(format!(
                "type `{other:?}` is not a struct type"
            ))),
        }
    }

    fn field_variable(&self, base: &str, field: &str) -> Result<Variable, Diagnostic> {
        if let Some(variable) = self.dotted_variable(base, field) {
            return Ok(variable);
        }
        let base_variable = self.variable(base)?;
        let base_type = self
            .variable_type(base)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{base}`")))?;
        let struct_name = self.struct_type_name(base_type)?;
        let layout = self
            .symbols
            .structs
            .get(&struct_name)
            .ok_or_else(|| Diagnostic::new(format!("unknown struct `{struct_name}`")))?;
        let field = layout.fields.get(field).ok_or_else(|| {
            Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
        })?;
        self.symbols
            .storage_at(base_variable.addr + field.offset, &field.ty)
    }

    fn field_type(&self, base: &str, field: &str) -> Result<Type, Diagnostic> {
        let key = format!("{base}.{field}");
        if let Some(ty) = self.named_value_type(&key) {
            return Ok(ty.clone());
        }
        let base_type = self
            .variable_type(base)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{base}`")))?;
        let struct_name = self.struct_type_name(base_type)?;
        let layout = self
            .symbols
            .structs
            .get(&struct_name)
            .ok_or_else(|| Diagnostic::new(format!("unknown struct `{struct_name}`")))?;
        layout
            .fields
            .get(field)
            .map(|field| field.ty.clone())
            .ok_or_else(|| {
                Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
            })
    }

    fn access_type(&self, path: &AccessPath) -> Result<Type, Diagnostic> {
        let mut ty = self
            .variable_type(&path.root)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?
            .clone();
        for segment in &path.segments {
            ty = match segment {
                AccessSegment::Field(field) => {
                    let struct_name = self.struct_type_name(&ty)?;
                    let layout = self.symbols.structs.get(&struct_name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown struct `{struct_name}`"))
                    })?;
                    layout
                        .fields
                        .get(field)
                        .map(|field| field.ty.clone())
                        .ok_or_else(|| {
                            Diagnostic::new(format!(
                                "struct `{struct_name}` has no field `{field}`"
                            ))
                        })?
                }
                AccessSegment::Index(index) => {
                    self.validate_array_index_type(index)?;
                    match self.symbols.resolved_type(&ty)? {
                        Type::Array { element, len } => {
                            self.validate_const_access_index_bounds(
                                index,
                                &len,
                                &access_path_summary(path),
                            )?;
                            *element
                        }
                        _ => {
                            return Err(Diagnostic::new(format!(
                                "value `{}` is not an array",
                                access_path_summary(path)
                            )));
                        }
                    }
                }
            };
        }
        Ok(ty)
    }

    fn const_access_variable(&self, path: &AccessPath) -> Result<Option<Variable>, Diagnostic> {
        let mut variable = self.variable(&path.root)?;
        let mut ty = self
            .variable_type(&path.root)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?
            .clone();

        for segment in &path.segments {
            match segment {
                AccessSegment::Field(field) => {
                    let struct_name = self.struct_type_name(&ty)?;
                    let layout = self.symbols.structs.get(&struct_name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown struct `{struct_name}`"))
                    })?;
                    let field_info = layout.fields.get(field).ok_or_else(|| {
                        Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
                    })?;
                    variable = self
                        .symbols
                        .storage_at(variable.addr + field_info.offset, &field_info.ty)?;
                    ty = field_info.ty.clone();
                }
                AccessSegment::Index(index) => {
                    self.validate_array_index_type(index)?;
                    let index_value = match self.symbols.eval_i64(index) {
                        Ok(value) => value,
                        Err(_) => return Ok(None),
                    };
                    let Type::Array { element, len } = self.symbols.resolved_type(&ty)? else {
                        return Err(Diagnostic::new(format!(
                            "value `{}` is not an array",
                            access_path_summary(path)
                        )));
                    };
                    let len = self.symbols.array_len(&len)?;
                    if index_value < 0 || index_value as u32 >= len {
                        return Err(Diagnostic::new(format!(
                            "array index {index_value} is out of bounds for `{}` length {len}",
                            access_path_summary(path)
                        )));
                    }
                    let element_size = self.symbols.type_size(&element)?;
                    variable = self
                        .symbols
                        .storage_at(variable.addr + index_value as u32 * element_size, &element)?;
                    ty = *element;
                }
            }
        }

        Ok(Some(variable))
    }

    fn emit_access_address(&mut self, path: &AccessPath) -> Result<(), Diagnostic> {
        let path = self.canonical_access_path(path);
        let path = &path;
        if path.segments.is_empty()
            && (self.named_value_type(&path.root).is_some()
                || self.symbols.embed_property_value(&path.root).is_some())
        {
            let value = self.value_for_width(&Expr::Ident(path.root.clone()), ValueWidth::U24)?;
            self.line(&format!("    ld hl, {value:06X}h"));
            return Ok(());
        }
        if let Some(variable) = self.const_access_variable(path)? {
            self.line(&format!("    ld hl, {:06X}h", variable.addr));
            return Ok(());
        }

        let root = self.variable(&path.root)?;
        let mut ty = self
            .variable_type(&path.root)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{}`", path.root)))?
            .clone();
        self.line(&format!("    ld hl, {:06X}h", root.addr));

        for segment in &path.segments {
            match segment {
                AccessSegment::Field(field) => {
                    let struct_name = self.struct_type_name(&ty)?;
                    let layout = self.symbols.structs.get(&struct_name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown struct `{struct_name}`"))
                    })?;
                    let field_info = layout.fields.get(field).ok_or_else(|| {
                        Diagnostic::new(format!("struct `{struct_name}` has no field `{field}`"))
                    })?;
                    let offset = field_info.offset;
                    let field_ty = field_info.ty.clone();
                    self.emit_add_hl_const(offset);
                    ty = field_ty;
                }
                AccessSegment::Index(index) => {
                    self.validate_array_index_type(index)?;
                    let Type::Array { element, len } = self.symbols.resolved_type(&ty)? else {
                        return Err(Diagnostic::new(format!(
                            "value `{}` is not an array",
                            access_path_summary(path)
                        )));
                    };
                    self.validate_const_access_index_bounds(
                        index,
                        &len,
                        &access_path_summary(path),
                    )?;
                    let element_size = self.symbols.type_size(&element)?;
                    let base_addr = self.alloc_var(ValueWidth::U24.bytes());
                    self.emit_store_hl(base_addr);
                    self.emit_expr_to_hl(index, ValueWidth::U24)?;
                    self.emit_scale_hl_by(element_size);
                    self.line("    push hl");
                    self.emit_load_hl(base_addr);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                    ty = *element;
                }
            }
        }
        Ok(())
    }

    fn emit_add_hl_const(&mut self, value: u32) {
        if value == 0 {
            return;
        }
        self.line("    push hl");
        self.line(&format!("    ld bc, {:06X}h", value & 0xFF_FFFF));
        self.line("    pop hl");
        self.line("    add hl, bc");
    }

    fn emit_scale_hl_by(&mut self, factor: u32) {
        match factor {
            0 | 1 => {}
            2 => self.line("    add hl, hl"),
            3 => {
                self.line("    push hl");
                self.line("    add hl, hl");
                self.line("    pop bc");
                self.line("    add hl, bc");
            }
            _ => {
                let index_value = self.alloc_var(ValueWidth::U24.bytes());
                self.emit_store_hl(index_value);
                for _ in 1..factor {
                    self.line("    push hl");
                    self.emit_load_hl(index_value);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                }
            }
        }
    }

    fn array_element_type(&self, name: &str) -> Result<Type, Diagnostic> {
        let Some(ty) = self.named_value_type(name) else {
            return Err(Diagnostic::new(format!("unknown array `{name}`")));
        };
        match self.symbols.resolved_type(ty)? {
            Type::Array { element, .. } | Type::Ptr(element) => Ok(*element),
            _ => Err(Diagnostic::new(format!(
                "`{name}` is not an array or pointer"
            ))),
        }
    }

    fn pointer_element_type(&self, name: &str) -> Result<Option<Type>, Diagnostic> {
        let Some(ty) = self.named_value_type(name) else {
            return Ok(None);
        };
        match self.symbols.resolved_type(ty)? {
            Type::Ptr(element) => Ok(Some(*element)),
            _ => Ok(None),
        }
    }

    fn validate_array_index_type(&self, index: &Expr) -> Result<(), Diagnostic> {
        if expr_is_untyped_literal(index) {
            let value = self.symbols.eval_i64(index)?;
            if !(0..=0xFF_FFFF).contains(&value) {
                return Err(Diagnostic::new(format!(
                    "array index value {value} is outside u24 range"
                )));
            }
            return Ok(());
        }

        let ty = self.symbols.resolved_type(&self.expr_type(index)?)?;
        if matches!(&ty, Type::Named(name) if matches!(name.as_str(), "u8" | "u16" | "u24")) {
            Ok(())
        } else {
            Err(Diagnostic::new(format!(
                "array index type `{}` is not supported; use u8, u16, or u24",
                type_display(&ty)
            )))
        }
    }

    fn validate_const_access_index_bounds(
        &self,
        index: &Expr,
        len: &Expr,
        path: &str,
    ) -> Result<(), Diagnostic> {
        let len = self.symbols.array_len(len)?;
        if let Ok(index_value) = self.symbols.eval_i64(index)
            && (index_value < 0 || index_value as u32 >= len)
        {
            return Err(Diagnostic::new(format!(
                "array index {index_value} is out of bounds for `{path}` length {len}",
            )));
        }
        Ok(())
    }

    fn pointer_pointee_size(&self, expr: &Expr) -> Result<Option<u32>, Diagnostic> {
        match self.expr_type(expr) {
            Ok(ty) => match self.symbols.resolved_type(&ty)? {
                Type::Ptr(inner) => Ok(Some(self.symbols.type_size(&inner)?)),
                _ => Ok(None),
            },
            Err(_) => Ok(None),
        }
    }

    fn emit_array_element_address(&mut self, name: &str, index: &Expr) -> Result<(), Diagnostic> {
        self.validate_array_index_type(index)?;
        if let Some(element_ty) = self.pointer_element_type(name)? {
            self.emit_expr_to_hl(&Expr::Ident(name.to_owned()), ValueWidth::U24)?;
            self.line("    push hl");
            self.emit_expr_to_hl(index, ValueWidth::U24)?;
            self.emit_scale_hl_by(self.symbols.type_size(&element_ty)?);
            self.line("    pop bc");
            self.line("    add hl, bc");
            return Ok(());
        }
        if let Some(element) = self.const_array_element_variable(name, index)? {
            self.line(&format!("    ld hl, {:06X}h", element.addr));
            return Ok(());
        }

        let (array, element_size, _) = self.array_info(name)?;
        self.emit_expr_to_hl(index, ValueWidth::U24)?;
        match element_size {
            1 => {}
            2 => self.line("    add hl, hl"),
            3 => {
                self.line("    push hl");
                self.line("    add hl, hl");
                self.line("    pop bc");
                self.line("    add hl, bc");
            }
            _ => {
                let index_value = self.alloc_var(ValueWidth::U24.bytes());
                self.emit_store_hl(index_value);
                for _ in 1..element_size {
                    self.line("    push hl");
                    self.emit_load_hl(index_value);
                    self.line("    pop bc");
                    self.line("    add hl, bc");
                }
            }
        }
        self.line("    push hl");
        self.line(&format!("    ld hl, {:06X}h", array.addr));
        self.line("    pop bc");
        self.line("    add hl, bc");
        Ok(())
    }

    fn emit_load_indexed_element_to_a(
        &mut self,
        name: &str,
        index: &Expr,
    ) -> Result<(), Diagnostic> {
        let width = self.array_element_width(name)?;
        if width != ValueWidth::U8 {
            return Err(Diagnostic::new(format!(
                "array `{name}` element is not u8-sized"
            )));
        }
        self.emit_array_element_address(name, index)?;
        self.line("    ld a, (hl)");
        Ok(())
    }

    fn emit_load_indexed_element_to_hl(
        &mut self,
        name: &str,
        index: &Expr,
    ) -> Result<(), Diagnostic> {
        let element_size = self.symbols.type_size(&self.array_element_type(name)?)?;
        if let Some(element) = self.const_array_element_variable(name, index)? {
            self.emit_load_width(element);
            return Ok(());
        }

        self.emit_array_element_address(name, index)?;
        match element_size {
            1 => {
                self.line("    ld a, (hl)");
                self.line("    ld hl, 000000h");
                self.line("    ld l, a");
            }
            2 | 3 => {
                let result = self.alloc_var(element_size);
                for offset in 0..element_size {
                    if offset != 0 {
                        self.line("    inc hl");
                    }
                    self.line("    ld a, (hl)");
                    self.line(&format!("    ld ({:06X}h), a", result.addr + offset));
                }
                self.emit_load_width(result);
            }
            _ => unreachable!("unsupported array element size"),
        }
        Ok(())
    }

    fn emit_index_assignment(
        &mut self,
        name: &str,
        index: &Expr,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let ty = self.array_element_type(name)?;
        if self.pointer_element_type(name)?.is_some() {
            return Err(Diagnostic::new(
                "index assignment through pointers is not supported; use explicit dereference",
            ));
        }
        if let Some(element) = self.const_array_element_variable(name, index)? {
            if op == AssignOp::Set {
                self.validate_expr_assignable_to_type(value, &ty)?;
                self.emit_storage_initializer(element, &ty, value)?;
                return Ok(());
            }
            self.ensure_compound_assignment_target(&ty, op)?;
            element.width()?;
            let signed = self.type_is_signed(&ty)?;
            self.emit_typed_assignment_value(element, &ty, op, value, signed)?;
            self.emit_store_width(element);
            return Ok(());
        }

        let element_size = self.symbols.type_size(&ty)?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        self.emit_array_element_address(name, index)?;
        self.emit_store_hl(addr);

        let element = self.symbols.storage_at(0, &ty)?;
        if op != AssignOp::Set {
            self.ensure_compound_assignment_target(&ty, op)?;
            element.width()?;
            let current = self.alloc_var(element_size);
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.alloc_var(element_size);
            let signed = self.type_is_signed(&ty)?;
            self.emit_typed_assignment_value(current, &ty, op, value, signed)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        if op == AssignOp::Set {
            self.validate_expr_assignable_to_type(value, &ty)?;
        }
        let stored = self.alloc_storage(&ty)?;
        self.emit_storage_initializer(stored, &ty, value)?;
        self.emit_load_hl(addr);
        self.emit_store_var_to_pointed_width(stored);
        Ok(())
    }

    fn emit_access_assignment(
        &mut self,
        path: &AccessPath,
        op: AssignOp,
        value: &Expr,
    ) -> Result<(), Diagnostic> {
        let path = self.canonical_access_path(path);
        let ty = self.access_type(&path)?;
        if let Some(variable) = self.const_access_variable(&path)? {
            if op == AssignOp::Set {
                self.validate_expr_assignable_to_type(value, &ty)?;
                self.emit_storage_initializer(variable, &ty, value)?;
                return Ok(());
            }
            self.ensure_compound_assignment_target(&ty, op)?;
            variable.width()?;
            let signed = self.type_is_signed(&ty)?;
            self.emit_typed_assignment_value(variable, &ty, op, value, signed)?;
            self.emit_store_width(variable);
            return Ok(());
        }

        let size = self.symbols.type_size(&ty)?;
        let addr = self.alloc_var(ValueWidth::U24.bytes());
        self.emit_access_address(&path)?;
        self.emit_store_hl(addr);

        if op != AssignOp::Set {
            self.ensure_compound_assignment_target(&ty, op)?;
            let current = self.alloc_var(size);
            current.width()?;
            self.emit_load_hl(addr);
            self.emit_load_pointed_width_into(current);
            let stored = self.alloc_var(size);
            let signed = self.type_is_signed(&ty)?;
            self.emit_typed_assignment_value(current, &ty, op, value, signed)?;
            self.emit_store_width(stored);
            self.emit_load_hl(addr);
            self.emit_store_var_to_pointed_width(stored);
            return Ok(());
        }

        self.validate_expr_assignable_to_type(value, &ty)?;
        let stored = self.alloc_storage(&ty)?;
        self.emit_storage_initializer(stored, &ty, value)?;
        self.emit_load_hl(addr);
        self.emit_store_var_to_pointed_width(stored);
        Ok(())
    }

    fn u8(&self, expr: &Expr) -> Result<u8, Diagnostic> {
        let value = self.symbols.eval_i64(expr)?;
        if !(0..=0xFF).contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {value} is outside u8 range"
            )));
        }
        Ok(value as u8)
    }

    fn u16(&self, expr: &Expr) -> Result<u16, Diagnostic> {
        let value = self.symbols.eval_i64(expr)?;
        if !(0..=0xFFFF).contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {value} is outside u16 range"
            )));
        }
        Ok(value as u16)
    }

    fn u24(&self, expr: &Expr) -> Result<u32, Diagnostic> {
        let value = self.symbols.eval_i64(expr)?;
        if !(0..=0xFF_FFFF).contains(&value) {
            return Err(Diagnostic::new(format!(
                "value {value} is outside u24 range"
            )));
        }
        Ok(value as u32)
    }

    fn value_for_width(&self, expr: &Expr, width: ValueWidth) -> Result<u32, Diagnostic> {
        let value = if is_integer_literal(expr) {
            let bits = u32::from(width.bytes()) * 8;
            let mask = (1_i128 << bits) - 1;
            (self.symbols.eval_i64(expr)? as i128 & mask) as u32
        } else {
            match width {
                ValueWidth::U8 => self.u8(expr).map(u32::from),
                ValueWidth::U16 => self.u16(expr).map(u32::from),
                ValueWidth::U24 => self.u24(expr),
            }?
        };
        self.validate_value_width_for_target(value, width)?;
        Ok(value)
    }

    fn eval_i64_with_local_constants(&self, expr: &Expr) -> Result<i64, Diagnostic> {
        match expr {
            Expr::Ident(name) => self
                .local_constant(name)
                .map(|constant| constant.value)
                .map(Ok)
                .unwrap_or_else(|| self.symbols.eval_i64(expr)),
            Expr::Unary { op, expr } => {
                let value = self.eval_i64_with_local_constants(expr)?;
                Ok(match op {
                    UnaryOp::Neg => value.wrapping_neg(),
                    UnaryOp::BitNot => !value,
                    UnaryOp::Not => i64::from(value == 0),
                })
            }
            Expr::Binary { left, op, right } => {
                let left_signed = self.expr_is_signed(left)?;
                let left_scale = self.pointer_pointee_size(left)?;
                let right_scale = self.pointer_pointee_size(right)?;
                let left = self.eval_i64_with_local_constants(left)?;
                let right = self.eval_i64_with_local_constants(right)?;
                Ok(match op {
                    BinaryOp::Mul => left.wrapping_mul(right),
                    BinaryOp::Div => trunc_div_or_zero(left, right),
                    BinaryOp::Mod => trunc_mod_or_zero(left, right),
                    BinaryOp::Add => match (left_scale, right_scale) {
                        (Some(scale), None) => left.wrapping_add(right.wrapping_mul(scale.into())),
                        (None, Some(scale)) => left.wrapping_mul(scale.into()).wrapping_add(right),
                        _ => left.wrapping_add(right),
                    },
                    BinaryOp::Sub => match (left_scale, right_scale) {
                        (Some(scale), None) => left.wrapping_sub(right.wrapping_mul(scale.into())),
                        _ => left.wrapping_sub(right),
                    },
                    BinaryOp::Shl => const_shl_or_zero(left, right),
                    BinaryOp::Shr => const_shr_or_zero(left, right, left_signed),
                    BinaryOp::Lt => i64::from(left < right),
                    BinaryOp::Le => i64::from(left <= right),
                    BinaryOp::Gt => i64::from(left > right),
                    BinaryOp::Ge => i64::from(left >= right),
                    BinaryOp::Eq => i64::from(left == right),
                    BinaryOp::Ne => i64::from(left != right),
                    BinaryOp::BitAnd => left & right,
                    BinaryOp::BitXor => left ^ right,
                    BinaryOp::BitOr => left | right,
                    BinaryOp::And => i64::from(left != 0 && right != 0),
                    BinaryOp::Or => i64::from(left != 0 || right != 0),
                })
            }
            Expr::Cast { expr, ty } => {
                let value = self.eval_i64_with_local_constants(expr)?;
                self.symbols.const_cast_value(value, ty)
            }
            _ => self.symbols.eval_i64(expr),
        }
    }

    fn local_constant_value_for_width(
        &self,
        name: &str,
        width: ValueWidth,
    ) -> Result<Option<u32>, Diagnostic> {
        let Some(constant) = self.local_constant(name) else {
            return Ok(None);
        };
        let source_width = self.symbols.type_width(&constant.ty)?;
        let value_width = if width.bytes() >= source_width.bytes() {
            source_width
        } else {
            width
        };
        let value = self.value_for_type(constant.value, &constant.ty, value_width)?;
        Ok(Some(value))
    }

    fn value_for_type(&self, value: i64, ty: &Type, width: ValueWidth) -> Result<u32, Diagnostic> {
        let resolved = self.symbols.resolved_type(ty)?;
        self.symbols.validate_value_for_type(value, &resolved)?;
        let bits = u32::from(width.bytes()) * 8;
        let mask = (1_i128 << bits) - 1;
        let value = ((value as i128) & mask) as u32;
        self.validate_value_width_for_target(value, width)?;
        Ok(value)
    }

    fn validate_value_width_for_target(
        &self,
        value: u32,
        width: ValueWidth,
    ) -> Result<(), Diagnostic> {
        if is_z80_family_16bit(self.cpu) && width == ValueWidth::U24 && value > 0xFFFF {
            return Err(Diagnostic::new(format!(
                "24-bit value 0x{value:06X} cannot be encoded for 16-bit target `{}`",
                self.cpu.as_str()
            )));
        }
        Ok(())
    }

    fn type_is_signed(&self, ty: &Type) -> Result<bool, Diagnostic> {
        Ok(type_is_signed(&self.symbols.resolved_type(ty)?))
    }

    fn expr_is_signed(&self, expr: &Expr) -> Result<bool, Diagnostic> {
        self.type_is_signed(&self.expr_type(expr)?)
    }

    fn expr_type(&self, expr: &Expr) -> Result<Type, Diagnostic> {
        match expr {
            Expr::Ident(name) => self
                .named_value_type(name)
                .cloned()
                .ok_or_else(|| Diagnostic::new(format!("unknown value `{name}`"))),
            Expr::Int(value) => {
                if (0..=0xFF).contains(value) {
                    Ok(Type::Named("u8".to_owned()))
                } else if (0..=0xFFFF).contains(value) {
                    Ok(Type::Named("u16".to_owned()))
                } else {
                    Ok(Type::Named("u24".to_owned()))
                }
            }
            Expr::TypedInt(_, ty) => Ok(ty.clone()),
            Expr::Char(_) | Expr::In(_) => Ok(Type::Named("u8".to_owned())),
            Expr::Bool(_) => Ok(Type::Named("bool".to_owned())),
            Expr::String(_) => Ok(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            Expr::Array(_) => Err(Diagnostic::new("array literal does not have scalar type")),
            Expr::Index { name, .. } => self.array_element_type(name),
            Expr::Field { base, field } => self
                .named_value_type(&format!("{base}.{field}"))
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| self.field_type(base, field)),
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty() {
                    if let Some(ty) = self.named_value_type(&path.root) {
                        return Ok(ty.clone());
                    }
                    if let Some(ty) = self.embed_property_type(&path.root) {
                        return Ok(ty);
                    }
                }
                self.access_type(&path)
            }
            Expr::AddressOfIndex { name, .. } => {
                Ok(Type::Ptr(Box::new(self.array_element_type(name)?)))
            }
            Expr::AddressOfField { base, field } => {
                Ok(Type::Ptr(Box::new(self.field_type(base, field)?)))
            }
            Expr::AddressOfAccess(path) => {
                let path = self.canonical_access_path(path);
                Ok(Type::Ptr(Box::new(self.access_type(&path)?)))
            }
            Expr::AddressOf(name) => {
                if self.symbols.functions.contains_key(name) {
                    return Ok(Type::Ptr(Box::new(self.function_value_type(name)?)));
                }
                if self.symbols.embeds.contains_key(name) {
                    return Ok(Type::Ptr(Box::new(Type::Named("u8".to_owned()))));
                }
                let Some(ty) = self.variable_type(name) else {
                    return Err(Diagnostic::new(format!("unknown variable `{name}`")));
                };
                Ok(Type::Ptr(Box::new(self.symbols.resolved_type(ty)?)))
            }
            Expr::Deref(ptr) => match self.symbols.resolved_type(&self.expr_type(ptr)?)? {
                Type::Ptr(inner) => Ok(*inner),
                Type::Named(name) if name == "ptr" => Err(Diagnostic::new(
                    "raw ptr dereference requires an explicit typed pointer cast",
                )),
                other => Err(Diagnostic::new(format!(
                    "cannot dereference non-pointer expression of type `{other:?}`"
                ))),
            },
            Expr::StructInit { ty, .. } => Ok(Type::Named(ty.clone())),
            Expr::Cast { ty, .. } => Ok(ty.clone()),
            Expr::BankedPointer { pointer, .. } => self.expr_type(pointer),
            Expr::Call { path, args } => {
                let name = path_text(path);
                if CATALOG.lookup(&name).is_some() {
                    self.intrinsic_result_type(&name, args)
                } else {
                    self.call_return_type(path)
                }
            }
            Expr::Unary { expr, op } => match op {
                UnaryOp::Not => {
                    self.ensure_expr_is_bool(expr, "logical operand")?;
                    Ok(Type::Named("bool".to_owned()))
                }
                UnaryOp::Neg | UnaryOp::BitNot => self.expr_type(expr),
            },
            Expr::Binary { left, op, right } => {
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) {
                    Ok(Type::Named("bool".to_owned()))
                } else if matches!(op, BinaryOp::Add | BinaryOp::Sub)
                    && self.pointer_pointee_size(left)?.is_some()
                {
                    self.expr_type(left)
                } else if *op == BinaryOp::Add && self.pointer_pointee_size(right)?.is_some() {
                    self.expr_type(right)
                } else if self.expr_width(left)? >= self.expr_width(right)? {
                    self.expr_type(left)
                } else {
                    self.expr_type(right)
                }
            }
        }
    }

    fn expr_width(&self, expr: &Expr) -> Result<ValueWidth, Diagnostic> {
        match expr {
            Expr::Ident(name) => {
                if let Some(variable) = self.variable_opt(name) {
                    variable.width()
                } else if let Some(ty) = self.named_value_type(name) {
                    self.symbols.type_width(ty)
                } else {
                    let value = self.symbols.eval_i64(expr)?;
                    if (0..=0xFF).contains(&value) {
                        Ok(ValueWidth::U8)
                    } else if (0..=0xFFFF).contains(&value) {
                        Ok(ValueWidth::U16)
                    } else {
                        Ok(ValueWidth::U24)
                    }
                }
            }
            Expr::Int(value) => {
                if (0..=0xFF).contains(value) {
                    Ok(ValueWidth::U8)
                } else if (0..=0xFFFF).contains(value) {
                    Ok(ValueWidth::U16)
                } else {
                    Ok(ValueWidth::U24)
                }
            }
            Expr::TypedInt(_, ty) => self.symbols.type_width(ty),
            Expr::Char(_) | Expr::Bool(_) | Expr::In(_) => Ok(ValueWidth::U8),
            Expr::String(_) => Ok(ValueWidth::U24),
            Expr::Array(_) => Err(Diagnostic::new("array literal does not have scalar width")),
            Expr::StructInit { ty, .. } => Err(Diagnostic::new(format!(
                "struct `{ty}` literal does not have scalar width"
            ))),
            Expr::Index { name, .. } => self.array_element_width(name),
            Expr::Field { base, field } => {
                let key = format!("{base}.{field}");
                if let Some(ty) = self.named_value_type(&key) {
                    self.symbols.type_width(ty)
                } else {
                    self.field_variable(base, field)?.width()
                }
            }
            Expr::Access(path) => {
                let path = self.canonical_access_path(path);
                if path.segments.is_empty() {
                    if let Some(ty) = self.named_value_type(&path.root) {
                        return self.symbols.type_width(ty);
                    }
                    if self.symbols.embed_property_value(&path.root).is_some() {
                        return Ok(ValueWidth::U24);
                    }
                }
                if let Some(variable) = self.const_access_variable(&path)? {
                    variable.width()
                } else {
                    self.symbols.type_width(&self.access_type(&path)?)
                }
            }
            Expr::AddressOfIndex { .. }
            | Expr::AddressOfField { .. }
            | Expr::AddressOfAccess(_)
            | Expr::AddressOf(_) => self.symbols.type_width(&self.expr_type(expr)?),
            Expr::Deref(ptr) => match self.symbols.resolved_type(&self.expr_type(ptr)?)? {
                Type::Ptr(inner) => self.symbols.type_width(&inner),
                Type::Named(name) if name == "ptr" => Err(Diagnostic::new(
                    "raw ptr dereference requires an explicit typed pointer cast",
                )),
                other => Err(Diagnostic::new(format!(
                    "cannot dereference non-pointer expression of type `{other:?}`"
                ))),
            },
            Expr::Cast { ty, .. } => self.symbols.type_width(ty),
            Expr::BankedPointer { pointer, .. } => self.expr_width(pointer),
            Expr::Call { path, args } => {
                let name = path_text(path);
                if CATALOG.lookup(&name).is_some() {
                    self.symbols
                        .type_width(&self.intrinsic_result_type(&name, args)?)
                } else {
                    self.call_return_width(path)
                }
            }
            Expr::Unary { expr, op } => match op {
                UnaryOp::Not => Ok(ValueWidth::U8),
                UnaryOp::Neg | UnaryOp::BitNot => self.expr_width(expr),
            },
            Expr::Binary { left, op, right } => {
                if is_comparison(*op) || matches!(op, BinaryOp::And | BinaryOp::Or) {
                    Ok(ValueWidth::U8)
                } else {
                    Ok(self.expr_width(left)?.max(self.expr_width(right)?))
                }
            }
        }
    }

    fn intrinsic_argument_types(&self, name: &str, args: &[Expr]) -> Result<Vec<Type>, Diagnostic> {
        let descriptor = CATALOG
            .lookup(name)
            .ok_or_else(|| Diagnostic::new(format!("unknown intrinsic `{name}`")))?;
        let mut types = args
            .iter()
            .map(|arg| {
                self.expr_type(arg)
                    .and_then(|ty| self.symbols.resolved_type(&ty))
            })
            .collect::<Result<Vec<_>, _>>()?;

        for (index, ty) in types.iter_mut().enumerate() {
            if !expr_is_untyped_literal(&args[index]) {
                continue;
            }
            let expected = match descriptor.operation {
                IntrinsicOperation::Mem(operation) => match (operation, index) {
                    (MemIntrinsic::CopyNonoverlapping | MemIntrinsic::Move, 2)
                    | (MemIntrinsic::Fill, 2)
                    | (MemIntrinsic::FindByte, 1)
                    | (MemIntrinsic::Compare, 2) => Some("u24"),
                    (MemIntrinsic::Fill, 1)
                    | (MemIntrinsic::FindByte, 2)
                    | (MemIntrinsic::Poke8, 1) => Some("u8"),
                    (MemIntrinsic::StoreLe16 | MemIntrinsic::StoreBe16, 1) => Some("u16"),
                    (MemIntrinsic::StoreLe24 | MemIntrinsic::StoreBe24, 1) => Some("u24"),
                    _ => None,
                },
                _ => None,
            };
            if let Some(expected) = expected {
                *ty = Type::Named(expected.to_owned());
            }
        }
        Ok(types)
    }

    fn resolve_intrinsic_call(
        &self,
        name: &str,
        args: &[Expr],
    ) -> Result<IntrinsicResolution, Diagnostic> {
        let types = self.intrinsic_argument_types(name, args)?;
        let constants = args
            .iter()
            .map(|arg| self.eval_i64_with_local_constants(arg).ok())
            .collect::<Vec<_>>();
        let resolution = CATALOG
            .validate_types_with_constants(name, &types, &constants)
            .map_err(|error| {
                if is_legacy_memory_intrinsic(name)
                    && matches!(
                        error,
                        IntrinsicError::ArgumentType { .. }
                            | IntrinsicError::MismatchedArguments { .. }
                    )
                {
                    Diagnostic::new("type mismatch")
                } else {
                    Diagnostic::new(error.to_string())
                }
            })?;

        for result_type in &resolution.result_types {
            if let Err(error) = self.symbols.type_width(result_type) {
                return Err(Diagnostic::new(format!(
                    "intrinsic `{}` result type `{result_type:?}` is unsupported by the eZ80 emitter: {}",
                    resolution.canonical_name(),
                    error.message
                )));
            }
        }
        self.validate_intrinsic_volatile_access(&resolution, args)?;
        Ok(resolution)
    }

    fn intrinsic_result_type(&self, name: &str, args: &[Expr]) -> Result<Type, Diagnostic> {
        let resolution = self.resolve_intrinsic_call(name, args)?;
        match resolution.result_types.as_slice() {
            [result] => Ok(result.clone()),
            [] => Err(Diagnostic::new(format!(
                "intrinsic `{}` does not return a value",
                resolution.canonical_name()
            ))),
            _ => Err(Diagnostic::new(format!(
                "intrinsic `{}` returns two values; use a two-destination binding",
                resolution.canonical_name()
            ))),
        }
    }

    fn emit_intrinsic_call(&mut self, name: &str, args: &[Expr]) -> Result<(), Diagnostic> {
        let resolution = self.resolve_intrinsic_call(name, args)?;
        match resolution.result_count() {
            ResultCount::Zero => self.emit_zero_result_intrinsic(
                resolution.canonical_name(),
                resolution.descriptor.operation,
                args,
            ),
            ResultCount::One => self.emit_intrinsic_value(name, args),
            ResultCount::Two => Err(Diagnostic::new(format!(
                "intrinsic `{}` returns two values; use a two-destination binding",
                resolution.canonical_name()
            ))),
        }
    }

    fn emit_intrinsic_value(&mut self, name: &str, args: &[Expr]) -> Result<(), Diagnostic> {
        let resolution = self.resolve_intrinsic_call(name, args)?;
        if resolution.result_count() != ResultCount::One {
            return Err(Diagnostic::new(format!(
                "intrinsic `{}` does not return one scalar value",
                resolution.canonical_name()
            )));
        }
        match resolution.descriptor.operation {
            IntrinsicOperation::Bits(operation) => self.emit_bits_intrinsic(name, operation, args),
            IntrinsicOperation::Int(operation) => self.emit_int_intrinsic(name, operation, args),
            IntrinsicOperation::Mem(operation) => {
                self.emit_memory_intrinsic_value(name, operation, args)
            }
        }
    }

    fn intrinsic_argument_temps(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Vec<Variable>, Diagnostic> {
        let types = self.intrinsic_argument_types(name, args)?;
        let mut temps = Vec::with_capacity(args.len());
        for (arg, ty) in args.iter().zip(types) {
            let width = self.symbols.type_width(&ty)?;
            let temp = self.alloc_var(width.bytes());
            self.emit_expr_to_type(arg, &ty)?;
            self.emit_store_width(temp);
            temps.push(temp);
        }
        Ok(temps)
    }

    fn validate_intrinsic_volatile_access(
        &self,
        resolution: &IntrinsicResolution,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        if resolution.descriptor.effects.volatile != VolatilePolicy::NonVolatileOnly {
            return Ok(());
        }
        let pointer_indices = match resolution.descriptor.operation {
            IntrinsicOperation::Mem(
                MemIntrinsic::CopyNonoverlapping
                | MemIntrinsic::Move
                | MemIntrinsic::Fill
                | MemIntrinsic::FindByte
                | MemIntrinsic::Compare,
            ) => &[0, 1][..],
            IntrinsicOperation::Mem(
                MemIntrinsic::LoadLe16
                | MemIntrinsic::LoadLe24
                | MemIntrinsic::LoadBe16
                | MemIntrinsic::LoadBe24
                | MemIntrinsic::StoreLe16
                | MemIntrinsic::StoreLe24
                | MemIntrinsic::StoreBe16
                | MemIntrinsic::StoreBe24,
            ) => &[0][..],
            _ => &[][..],
        };
        for index in pointer_indices.iter().copied() {
            let Some(arg) = args.get(index) else {
                continue;
            };
            let ty = self.symbols.resolved_type(&self.expr_type(arg)?)?;
            if !matches!(ty, Type::Ptr(_)) {
                continue;
            }
            let Ok(address) = self.eval_i64_with_local_constants(arg) else {
                continue;
            };
            let address = u32::try_from(address).ok();
            if address.is_some_and(|address| {
                self.symbols
                    .volatile_ranges
                    .iter()
                    .any(|(start, end)| address < *end && address.saturating_add(1) > *start)
            }) {
                return Err(Diagnostic::new(format!(
                    "intrinsic `{}` cannot access volatile memory",
                    resolution.canonical_name()
                )));
            }
        }
        Ok(())
    }

    fn emit_zero_result_intrinsic(
        &mut self,
        name: &str,
        operation: IntrinsicOperation,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        match operation {
            IntrinsicOperation::Mem(operation) => {
                self.emit_memory_intrinsic_zero(name, operation, args)
            }
            _ => Err(Diagnostic::new(
                "intrinsic does not produce a zero-result operation",
            )),
        }
    }

    fn emit_bits_intrinsic(
        &mut self,
        name: &str,
        operation: BitsIntrinsic,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        let types = self.intrinsic_argument_types(name, args)?;
        let value_type = self.symbols.resolved_type(&types[0])?;
        let value_width = self.symbols.type_width(&value_type)?;
        let temps = self.intrinsic_argument_temps(name, args)?;
        match operation {
            BitsIntrinsic::RotateLeft | BitsIntrinsic::RotateRight => {
                let count = temps[1];
                self.emit_reduce_memory_mod_const(count, u32::from(value_width.bytes()) * 8);
                let done = self.next_label("intrinsic_rotate_done");
                self.line(&format!("{done}_loop:"));
                self.emit_jump_if_memory_zero(count, &done);
                self.emit_rotate_memory_once(temps[0], operation == BitsIntrinsic::RotateLeft);
                self.emit_decrement_memory(count);
                self.line(&format!("    jp {done}_loop"));
                self.line(&format!("{done}:"));
                self.emit_load_width(temps[0]);
            }
            BitsIntrinsic::Test => {
                let bit = self.eval_i64_with_local_constants(&args[1])? as u32;
                let byte_offset = bit / 8;
                let bit_offset = bit % 8;
                self.line(&format!("    ld a, ({:06X}h)", temps[0].addr + byte_offset));
                let true_label = self.next_label("intrinsic_bit_true");
                let end_label = self.next_label("intrinsic_bit_end");
                if supports_z80_bit_instructions(self.cpu) {
                    self.line(&format!("    bit {bit_offset}, a"));
                    self.line(&format!("    jp nz, {true_label}"));
                } else {
                    self.line(&format!("    and {:02X}h", 1_u8 << bit_offset));
                    self.line(&format!("    jp nz, {true_label}"));
                }
                self.line("    ld a, 00h");
                self.line(&format!("    jp {end_label}"));
                self.line(&format!("{true_label}:"));
                self.line("    ld a, 01h");
                self.line(&format!("{end_label}:"));
            }
            BitsIntrinsic::Set | BitsIntrinsic::Clear | BitsIntrinsic::Toggle => {
                let bit = self.eval_i64_with_local_constants(&args[1])? as u32;
                let byte_offset = bit / 8;
                let bit_offset = bit % 8;
                self.line(&format!("    ld a, ({:06X}h)", temps[0].addr + byte_offset));
                match operation {
                    BitsIntrinsic::Set if supports_z80_bit_instructions(self.cpu) => {
                        self.line(&format!("    set {bit_offset}, a"));
                    }
                    BitsIntrinsic::Clear if supports_z80_bit_instructions(self.cpu) => {
                        self.line(&format!("    res {bit_offset}, a"));
                    }
                    BitsIntrinsic::Set => self.line(&format!("    or {:02X}h", 1_u8 << bit_offset)),
                    BitsIntrinsic::Clear => {
                        self.line(&format!("    and {:02X}h", !(1_u8 << bit_offset)))
                    }
                    BitsIntrinsic::Toggle => {
                        self.line(&format!("    xor {:02X}h", 1_u8 << bit_offset))
                    }
                    _ => unreachable!("bit operation handled above"),
                }
                self.line(&format!("    ld ({:06X}h), a", temps[0].addr + byte_offset));
                self.emit_load_width(temps[0]);
            }
            BitsIntrinsic::Extract => {
                let offset = self.eval_i64_with_local_constants(&args[1])? as u8;
                let width = self.eval_i64_with_local_constants(&args[2])? as u8;
                self.emit_shift_memory(temps[0], BinaryOp::Shr, offset, false)?;
                let mask = (1_u32 << width) - 1;
                self.emit_mask_variable(temps[0], mask);
                self.emit_load_width(temps[0]);
            }
            BitsIntrinsic::Insert => {
                let offset = self.eval_i64_with_local_constants(&args[2])? as u8;
                let width = self.eval_i64_with_local_constants(&args[3])? as u8;
                self.emit_shift_memory(temps[1], BinaryOp::Shl, offset, false)?;
                let field_mask = ((1_u32 << width) - 1) << offset;
                self.emit_mask_variable(temps[1], field_mask);
                self.emit_mask_variable(temps[0], !field_mask);
                self.emit_or_memory(temps[0], temps[1]);
                self.emit_load_width(temps[0]);
            }
            BitsIntrinsic::ByteSwap => {
                let result = self.alloc_var(value_width.bytes());
                for offset in 0..value_width.bytes() {
                    let source = temps[0].addr + u32::from(value_width.bytes() - 1 - offset);
                    self.line(&format!("    ld a, ({source:06X}h)"));
                    self.line(&format!(
                        "    ld ({:06X}h), a",
                        result.addr + u32::from(offset)
                    ));
                }
                self.emit_load_width(result);
            }
            BitsIntrinsic::Reverse => {
                let result = self.alloc_var(value_width.bytes());
                for offset in 0..value_width.bytes() {
                    let source = temps[0].addr + u32::from(offset);
                    let destination = result.addr + u32::from(value_width.bytes() - 1 - offset);
                    let byte = self.alloc_var(1u32);
                    let reversed = self.alloc_var(1u32);
                    self.line(&format!("    ld a, ({source:06X}h)"));
                    self.emit_store_a(byte);
                    self.emit_zero_bytes(reversed);
                    for _ in 0..8 {
                        self.emit_load_a(byte);
                        self.line("    srl a");
                        self.emit_store_a(byte);
                        self.emit_load_a(reversed);
                        self.line("    rla");
                        self.emit_store_a(reversed);
                    }
                    self.emit_load_a(reversed);
                    self.line(&format!("    ld ({destination:06X}h), a"));
                }
                self.emit_load_width(result);
            }
            BitsIntrinsic::CountOnes => {
                self.emit_count_ones(temps[0], value_width.bytes());
            }
            BitsIntrinsic::LeadingZeros => {
                self.emit_count_leading_zeros(temps[0], value_width.bytes());
            }
            BitsIntrinsic::TrailingZeros => {
                self.emit_count_trailing_zeros(temps[0], value_width.bytes());
            }
        }
        Ok(())
    }

    fn emit_reduce_memory_mod_const(&mut self, variable: Variable, modulus: u32) {
        let done = self.next_label("intrinsic_mod_done");
        let loop_label = self.next_label("intrinsic_mod_loop");
        self.line(&format!("{loop_label}:"));
        if variable.size == 1 {
            self.emit_load_a(variable);
            self.line(&format!("    cp {modulus:02X}h"));
            self.line(&format!("    jp c, {done}"));
        } else {
            self.emit_load_width(variable);
            self.line("    push hl");
            self.line(&format!("    ld de, {modulus:06X}h"));
            self.line("    pop hl");
            self.emit_compare_hl_de();
            self.line(&format!("    jp c, {done}"));
        }
        for _ in 0..modulus {
            self.emit_decrement_memory(variable);
        }
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done}:"));
    }

    fn emit_rotate_memory_once(&mut self, variable: Variable, left: bool) {
        if variable.size == 1 {
            self.emit_load_a(variable);
            self.line(if left { "    rlca" } else { "    rrca" });
            self.emit_store_a(variable);
            return;
        }
        if left {
            self.emit_load_a(variable);
            self.line("    add a, a");
            self.emit_store_a(variable);
            for offset in 1..variable.size {
                let address = variable.addr + offset;
                self.line(&format!("    ld a, ({address:06X}h)"));
                self.line("    rla");
                self.line(&format!("    ld ({address:06X}h), a"));
            }
            self.emit_load_a(variable);
            self.line("    rla");
            self.emit_store_a(variable);
        } else {
            let high = variable.addr + variable.size - 1;
            self.line(&format!("    ld a, ({high:06X}h)"));
            self.line("    srl a");
            self.line(&format!("    ld ({high:06X}h), a"));
            for offset in (0..variable.size - 1).rev() {
                let address = variable.addr + offset;
                self.line(&format!("    ld a, ({address:06X}h)"));
                self.line("    rra");
                self.line(&format!("    ld ({address:06X}h), a"));
            }
            let wrapped = self.next_label("intrinsic_rotate_right_wrapped");
            let done = self.next_label("intrinsic_rotate_right_done");
            self.line(&format!("    jp c, {wrapped}"));
            self.line(&format!("    jp {done}"));
            self.line(&format!("{wrapped}:"));
            self.line(&format!("    ld a, ({high:06X}h)"));
            self.line("    or 80h");
            self.line(&format!("    ld ({high:06X}h), a"));
            self.line(&format!("{done}:"));
        }
    }

    fn emit_mask_variable(&mut self, variable: Variable, mask: u32) {
        for offset in 0..variable.size {
            let byte_mask = ((mask >> (offset * 8)) & 0xFF) as u8;
            if byte_mask == 0xFF {
                continue;
            }
            if byte_mask == 0 {
                self.line("    xor a");
            } else {
                self.line(&format!("    ld a, ({:06X}h)", variable.addr + offset));
                self.line(&format!("    and {byte_mask:02X}h"));
            }
            self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
        }
    }

    fn emit_or_memory(&mut self, destination: Variable, source: Variable) {
        for offset in 0..destination.size {
            self.line(&format!("    ld a, ({:06X}h)", destination.addr + offset));
            self.line("    ld b, a");
            if offset < source.size {
                self.line(&format!("    ld a, ({:06X}h)", source.addr + offset));
                self.line("    or b");
            } else {
                self.line("    ld a, b");
            }
            self.line(&format!("    ld ({:06X}h), a", destination.addr + offset));
        }
    }

    fn emit_count_ones(&mut self, source: Variable, byte_count: u8) {
        let count = self.alloc_var(1u32);
        let byte = self.alloc_var(1u32);
        self.emit_zero_bytes(count);
        for offset in 0..byte_count {
            self.line(&format!(
                "    ld a, ({:06X}h)",
                source.addr + u32::from(offset)
            ));
            self.emit_store_a(byte);
            for _ in 0..8 {
                self.emit_load_a(byte);
                self.line("    srl a");
                self.emit_store_a(byte);
                let skip = self.next_label("intrinsic_count_skip");
                self.line(&format!("    jp nc, {skip}"));
                self.emit_increment_memory(count);
                self.line(&format!("{skip}:"));
            }
        }
        self.emit_load_a(count);
    }

    fn emit_count_leading_zeros(&mut self, source: Variable, byte_count: u8) {
        let count = self.alloc_var(1u32);
        let byte = self.alloc_var(1u32);
        self.emit_zero_bytes(count);
        let done = self.next_label("intrinsic_leading_done");
        for offset in (0..byte_count).rev() {
            self.line(&format!(
                "    ld a, ({:06X}h)",
                source.addr + u32::from(offset)
            ));
            self.emit_store_a(byte);
            for _ in 0..8 {
                self.emit_load_a(byte);
                self.line("    add a, a");
                self.emit_store_a(byte);
                self.line(&format!("    jp c, {done}"));
                self.emit_increment_memory(count);
            }
        }
        self.line(&format!("{done}:"));
        self.emit_load_a(count);
    }

    fn emit_count_trailing_zeros(&mut self, source: Variable, byte_count: u8) {
        let count = self.alloc_var(1u32);
        let byte = self.alloc_var(1u32);
        self.emit_zero_bytes(count);
        let done = self.next_label("intrinsic_trailing_done");
        for offset in 0..byte_count {
            self.line(&format!(
                "    ld a, ({:06X}h)",
                source.addr + u32::from(offset)
            ));
            self.emit_store_a(byte);
            for _ in 0..8 {
                self.emit_load_a(byte);
                self.line("    srl a");
                self.emit_store_a(byte);
                self.line(&format!("    jp c, {done}"));
                self.emit_increment_memory(count);
            }
        }
        self.line(&format!("{done}:"));
        self.emit_load_a(count);
    }

    fn emit_int_intrinsic(
        &mut self,
        name: &str,
        operation: IntIntrinsic,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        let types = self.intrinsic_argument_types(name, args)?;
        let first_type = self.symbols.resolved_type(&types[0])?;
        let first_info = crate::intrinsics::integer_info(&first_type)
            .expect("catalog accepted an integer intrinsic operand");
        let temps = self.intrinsic_argument_temps(name, args)?;
        match operation {
            IntIntrinsic::WideningMul => {
                let result_type = self.intrinsic_result_type(name, args)?;
                let result_width = self.symbols.type_width(&result_type)?;
                let product = self.alloc_var(temps[0].size + temps[1].size);
                self.emit_full_product(temps[0], temps[1], product, first_info.signed);
                let result = self.alloc_var(result_width.bytes());
                self.emit_copy_bytes(product, 0, result, 0, result.size);
                self.emit_load_width(result);
            }
            IntIntrinsic::MulHigh => {
                let product = self.alloc_var(temps[0].size * 2);
                self.emit_full_product(temps[0], temps[1], product, first_info.signed);
                let result = self.alloc_var(temps[0].size);
                self.emit_copy_bytes(product, temps[0].size, result, 0, result.size);
                self.emit_load_width(result);
            }
            IntIntrinsic::SaturatingAdd | IntIntrinsic::SaturatingSub => {
                let result = self.alloc_var(temps[0].size);
                self.emit_copy_bytes(temps[0], 0, result, 0, result.size);
                self.emit_saturating_memory_op(
                    result,
                    temps[0],
                    temps[1],
                    first_info.signed,
                    operation == IntIntrinsic::SaturatingSub,
                );
                self.emit_load_width(result);
            }
            IntIntrinsic::Divmod
            | IntIntrinsic::AddCarry
            | IntIntrinsic::SubBorrow
            | IntIntrinsic::FullMul => {
                return Err(Diagnostic::new(format!(
                    "intrinsic `{}` requires a two-destination binding",
                    CATALOG
                        .lookup(name)
                        .map(|descriptor| descriptor.canonical_name)
                        .unwrap_or(name)
                )));
            }
        }
        Ok(())
    }

    fn emit_copy_bytes(
        &mut self,
        source: Variable,
        source_offset: u32,
        destination: Variable,
        destination_offset: u32,
        count: u32,
    ) {
        for offset in 0..count {
            self.line(&format!(
                "    ld a, ({:06X}h)",
                source.addr + source_offset + offset
            ));
            self.line(&format!(
                "    ld ({:06X}h), a",
                destination.addr + destination_offset + offset
            ));
        }
    }

    fn emit_zero_bytes(&mut self, variable: Variable) {
        self.line("    xor a");
        for offset in 0..variable.size {
            self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
        }
    }

    fn emit_add_memory(&mut self, destination: Variable, source: Variable) {
        for offset in 0..destination.size {
            self.line(&format!("    ld a, ({:06X}h)", destination.addr + offset));
            self.line("    ld b, a");
            if offset < source.size {
                self.line(&format!("    ld a, ({:06X}h)", source.addr + offset));
            } else {
                self.line("    xor a");
            }
            if offset == 0 {
                self.line("    add a, b");
            } else {
                self.line("    adc a, b");
            }
            self.line(&format!("    ld ({:06X}h), a", destination.addr + offset));
        }
    }

    fn emit_sub_memory(&mut self, destination: Variable, source: Variable) {
        for offset in 0..destination.size {
            self.line(&format!("    ld a, ({:06X}h)", destination.addr + offset));
            self.line("    ld b, a");
            if offset < source.size {
                self.line(&format!("    ld a, ({:06X}h)", source.addr + offset));
            } else {
                self.line("    xor a");
            }
            if offset == 0 {
                self.line("    ld c, a");
                self.line("    ld a, b");
                self.line("    sub c");
            } else {
                self.line("    ld c, a");
                self.line("    ld a, b");
                self.line("    sbc a, c");
            }
            self.line(&format!("    ld ({:06X}h), a", destination.addr + offset));
        }
    }

    fn emit_full_product(
        &mut self,
        first: Variable,
        second: Variable,
        product: Variable,
        signed: bool,
    ) {
        let negative = self.alloc_var(1u32);
        if signed {
            self.emit_zero_bytes(negative);
            self.emit_abs_signed_variable(first, Some(negative), None);
            self.emit_abs_signed_variable(second, Some(negative), None);
        }
        let multiplicand = self.alloc_var(product.size);
        let multiplier = self.alloc_var(second.size);
        self.emit_zero_bytes(multiplicand);
        self.emit_copy_bytes(first, 0, multiplicand, 0, first.size);
        self.emit_copy_bytes(second, 0, multiplier, 0, second.size);
        self.emit_zero_bytes(product);

        for _ in 0..u32::from(second.size) * 8 {
            let skip = self.next_label("intrinsic_mul_skip");
            self.line(&format!("    ld a, ({:06X}h)", multiplier.addr));
            self.line("    and 01h");
            self.line(&format!("    jp z, {skip}"));
            self.emit_add_memory(product, multiplicand);
            self.line(&format!("{skip}:"));
            self.emit_shift_memory_left_once(multiplicand);
            self.emit_shift_memory_right_once(multiplier, false);
        }
        if signed {
            let done = self.next_label("intrinsic_mul_signed_done");
            self.emit_load_a(negative);
            self.line("    or a");
            self.line(&format!("    jp z, {done}"));
            self.emit_negate_memory(product);
            self.line(&format!("{done}:"));
        }
    }

    fn emit_saturating_memory_op(
        &mut self,
        result: Variable,
        left: Variable,
        right: Variable,
        signed: bool,
        subtract: bool,
    ) {
        let saturated = self.next_label("intrinsic_saturated");
        let done = self.next_label("intrinsic_saturating_done");
        if subtract {
            self.emit_sub_memory(result, right);
        } else {
            self.emit_add_memory(result, right);
        }
        if !signed {
            self.line(&format!("    jp c, {saturated}"));
            self.line(&format!("    jp {done}"));
            self.line(&format!("{saturated}:"));
            if subtract {
                self.emit_zero_bytes(result);
            } else {
                self.emit_fill_bytes(result, 0xFF);
            }
            self.line(&format!("{done}:"));
            return;
        }

        let left_negative = self.next_label("intrinsic_left_negative");
        let right_negative = self.next_label("intrinsic_right_negative");
        let positive_overflow = self.next_label("intrinsic_positive_overflow");
        let negative_overflow = self.next_label("intrinsic_negative_overflow");
        let no_overflow = self.next_label("intrinsic_no_overflow");

        if subtract {
            self.emit_jump_if_memory_sign(left, &left_negative);
            self.emit_jump_if_memory_sign(right, &right_negative);
            self.line(&format!("    jp {no_overflow}"));
            self.line(&format!("{right_negative}:"));
            self.emit_jump_if_memory_sign(result, &positive_overflow);
            self.line(&format!("    jp {no_overflow}"));
            self.line(&format!("{left_negative}:"));
            self.emit_jump_if_memory_sign(right, &no_overflow);
            self.emit_jump_if_memory_sign(result, &no_overflow);
            self.line(&format!("    jp {negative_overflow}"));
        } else {
            self.emit_jump_if_memory_sign(left, &left_negative);
            self.emit_jump_if_memory_sign(right, &no_overflow);
            self.emit_jump_if_memory_sign(result, &positive_overflow);
            self.line(&format!("    jp {no_overflow}"));
            self.line(&format!("{left_negative}:"));
            self.emit_jump_if_memory_sign(right, &no_overflow);
            self.emit_jump_if_memory_sign(result, &no_overflow);
            self.line(&format!("    jp {negative_overflow}"));
        }
        self.line(&format!("{no_overflow}:"));
        self.line(&format!("    jp {done}"));
        self.line(&format!("{positive_overflow}:"));
        self.emit_signed_max_bytes(result);
        self.line(&format!("    jp {done}"));
        self.line(&format!("{negative_overflow}:"));
        self.emit_signed_min_bytes(result);
        self.line(&format!("{done}:"));
    }

    fn emit_fill_bytes(&mut self, variable: Variable, value: u8) {
        self.line(&format!("    ld a, {value:02X}h"));
        for offset in 0..variable.size {
            self.line(&format!("    ld ({:06X}h), a", variable.addr + offset));
        }
    }

    fn emit_jump_if_memory_sign(&mut self, variable: Variable, negative_label: &str) {
        let address = variable.addr + variable.size - 1;
        self.line(&format!("    ld a, ({address:06X}h)"));
        self.line("    add a, a");
        self.line(&format!("    jp c, {negative_label}"));
    }

    fn emit_signed_max_bytes(&mut self, variable: Variable) {
        self.emit_fill_bytes(variable, 0xFF);
        self.line("    ld a, 7Fh");
        self.line(&format!(
            "    ld ({:06X}h), a",
            variable.addr + variable.size - 1
        ));
    }

    fn emit_signed_min_bytes(&mut self, variable: Variable) {
        self.emit_zero_bytes(variable);
        self.line("    ld a, 80h");
        self.line(&format!(
            "    ld ({:06X}h), a",
            variable.addr + variable.size - 1
        ));
    }

    fn emit_memory_intrinsic_value(
        &mut self,
        name: &str,
        operation: MemIntrinsic,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        match operation {
            MemIntrinsic::Peek8 => self.emit_mem_peek8(args),
            MemIntrinsic::Compare => self.emit_memory_compare(name, args),
            MemIntrinsic::LoadLe16
            | MemIntrinsic::LoadLe24
            | MemIntrinsic::LoadBe16
            | MemIntrinsic::LoadBe24 => self.emit_memory_load(name, operation, args),
            MemIntrinsic::CopyNonoverlapping
            | MemIntrinsic::Move
            | MemIntrinsic::Fill
            | MemIntrinsic::FindByte
            | MemIntrinsic::StoreLe16
            | MemIntrinsic::StoreLe24
            | MemIntrinsic::StoreBe16
            | MemIntrinsic::StoreBe24
            | MemIntrinsic::Poke8 => Err(Diagnostic::new(format!(
                "intrinsic `{}` does not return one scalar value",
                CATALOG
                    .lookup(name)
                    .map(|descriptor| descriptor.canonical_name)
                    .unwrap_or(name)
            ))),
        }
    }

    fn emit_memory_intrinsic_zero(
        &mut self,
        name: &str,
        operation: MemIntrinsic,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        match operation {
            MemIntrinsic::CopyNonoverlapping => {
                self.reject_known_copy_overlap(args)?;
                self.emit_memcpy(args)
            }
            MemIntrinsic::Move => self.emit_memory_move(name, args),
            MemIntrinsic::Fill => self.emit_memset(args),
            MemIntrinsic::StoreLe16
            | MemIntrinsic::StoreLe24
            | MemIntrinsic::StoreBe16
            | MemIntrinsic::StoreBe24 => self.emit_memory_store(name, operation, args),
            MemIntrinsic::Poke8 => self.emit_mem_poke8(args),
            MemIntrinsic::Peek8
            | MemIntrinsic::FindByte
            | MemIntrinsic::Compare
            | MemIntrinsic::LoadLe16
            | MemIntrinsic::LoadLe24
            | MemIntrinsic::LoadBe16
            | MemIntrinsic::LoadBe24 => Err(Diagnostic::new(format!(
                "intrinsic `{}` does not produce a zero-result operation",
                CATALOG
                    .lookup(name)
                    .map(|descriptor| descriptor.canonical_name)
                    .unwrap_or(name)
            ))),
        }
    }

    fn reject_known_copy_overlap(&self, args: &[Expr]) -> Result<(), Diagnostic> {
        let Some(destination) = args.first().and_then(|arg| self.symbols.eval_i64(arg).ok()) else {
            return Ok(());
        };
        let Some(source) = args.get(1).and_then(|arg| self.symbols.eval_i64(arg).ok()) else {
            return Ok(());
        };
        let Some(length) = args
            .get(2)
            .and_then(|arg| self.eval_i64_with_local_constants(arg).ok())
        else {
            return Ok(());
        };
        if destination < 0 || source < 0 || length < 0 {
            return Ok(());
        }
        let destination_end = destination.saturating_add(length);
        let source_end = source.saturating_add(length);
        if destination < source_end && source < destination_end {
            return Err(Diagnostic::new(
                "intrinsic `ezra.mem.copy_nonoverlapping` source and destination ranges overlap",
            ));
        }
        Ok(())
    }

    fn emit_memory_load(
        &mut self,
        name: &str,
        operation: MemIntrinsic,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        let types = self.intrinsic_argument_types(name, args)?;
        let result_type = self.intrinsic_result_type(name, args)?;
        let result_width = self.symbols.type_width(&result_type)?;
        let pointer_width = self.symbols.type_width(&types[0])?;
        let temps = self.intrinsic_argument_temps(name, args)?;
        let result = self.alloc_var(result_width.bytes());
        let little_endian = matches!(operation, MemIntrinsic::LoadLe16 | MemIntrinsic::LoadLe24);
        for offset in 0..result.size {
            self.emit_load_width(temps[0]);
            self.line("    ld a, (hl)");
            let result_offset = if little_endian {
                offset
            } else {
                result.size - 1 - offset
            };
            self.line(&format!("    ld ({:06X}h), a", result.addr + result_offset));
            self.emit_increment_memory(temps[0]);
        }
        debug_assert_eq!(pointer_width, self.symbols.type_width(&types[0]).unwrap());
        self.emit_load_width(result);
        Ok(())
    }

    fn emit_memory_store(
        &mut self,
        name: &str,
        operation: MemIntrinsic,
        args: &[Expr],
    ) -> Result<(), Diagnostic> {
        let types = self.intrinsic_argument_types(name, args)?;
        let temps = self.intrinsic_argument_temps(name, args)?;
        let little_endian = matches!(operation, MemIntrinsic::StoreLe16 | MemIntrinsic::StoreLe24);
        let value_size = temps[1].size;
        for offset in 0..value_size {
            self.emit_load_width(temps[0]);
            let value_offset = if little_endian {
                offset
            } else {
                value_size - 1 - offset
            };
            self.line(&format!(
                "    ld a, ({:06X}h)",
                temps[1].addr + value_offset
            ));
            self.line("    ld (hl), a");
            self.emit_increment_memory(temps[0]);
        }
        debug_assert_eq!(value_size, self.symbols.type_size(&types[1]).unwrap());
        Ok(())
    }

    fn emit_memory_compare(&mut self, name: &str, args: &[Expr]) -> Result<(), Diagnostic> {
        let temps = self.intrinsic_argument_temps(name, args)?;
        let byte = self.alloc_var(1u32);
        let loop_label = self.next_label("intrinsic_compare_loop");
        let equal = self.next_label("intrinsic_compare_equal");
        let less = self.next_label("intrinsic_compare_less");
        let done = self.next_label("intrinsic_compare_done");
        self.line(&format!("{loop_label}:"));
        self.emit_jump_if_memory_zero(temps[2], &equal);
        self.emit_load_width(temps[0]);
        self.line("    ld a, (hl)");
        self.emit_store_a(byte);
        self.emit_load_width(temps[1]);
        self.line("    ld a, (hl)");
        self.line("    ld b, a");
        self.emit_load_a(byte);
        self.line("    cp b");
        self.line(&format!("    jp z, {equal}"));
        self.line(&format!("    jp c, {less}"));
        self.line("    ld a, 01h");
        self.line(&format!("    jp {done}"));
        self.line(&format!("{less}:"));
        self.line("    ld a, FFh");
        self.line(&format!("    jp {done}"));
        self.line(&format!("{equal}:"));
        self.emit_jump_if_memory_zero(temps[2], &done);
        self.emit_increment_memory(temps[0]);
        self.emit_increment_memory(temps[1]);
        self.emit_decrement_memory(temps[2]);
        self.line(&format!("    jp {loop_label}"));
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_memory_move(&mut self, name: &str, args: &[Expr]) -> Result<(), Diagnostic> {
        let temps = self.intrinsic_argument_temps(name, args)?;
        let empty = self.next_label("intrinsic_move_empty");
        let forward = self.next_label("intrinsic_move_forward");
        let backward = self.next_label("intrinsic_move_backward");
        let done = self.next_label("intrinsic_move_done");
        self.emit_jump_if_memory_zero(temps[2], &empty);
        self.emit_compare_memory(temps[0], temps[1]);
        self.line(&format!("    jp c, {forward}"));
        self.line(&format!("    jp z, {forward}"));
        self.line(&format!("    jp {backward}"));
        self.line(&format!("{forward}:"));
        self.emit_memory_copy_loop(temps[1], temps[0], temps[2], false, &done);
        self.line(&format!("{backward}:"));
        self.emit_add_memory(temps[1], temps[2]);
        self.emit_add_memory(temps[0], temps[2]);
        self.emit_decrement_memory(temps[1]);
        self.emit_decrement_memory(temps[0]);
        self.emit_memory_copy_loop(temps[1], temps[0], temps[2], true, &done);
        self.line(&format!("{empty}:"));
        self.line(&format!("{done}:"));
        Ok(())
    }

    fn emit_compare_memory(&mut self, left: Variable, right: Variable) {
        if left.size == 1 {
            self.emit_load_a(right);
            self.line("    ld b, a");
            self.emit_load_a(left);
            self.line("    cp b");
        } else {
            self.emit_load_width(left);
            self.line("    push hl");
            self.emit_load_width(right);
            self.line("    ex de, hl");
            self.line("    pop hl");
            self.emit_compare_hl_de();
        }
    }

    fn emit_memory_copy_loop(
        &mut self,
        source: Variable,
        destination: Variable,
        length: Variable,
        backward: bool,
        done: &str,
    ) {
        let byte = self.alloc_var(1u32);
        let loop_label = self.next_label("intrinsic_move_loop");
        self.line(&format!("{loop_label}:"));
        self.emit_jump_if_memory_zero(length, done);
        self.emit_load_width(source);
        self.line("    ld a, (hl)");
        self.emit_store_a(byte);
        self.emit_load_width(destination);
        self.emit_load_a(byte);
        self.line("    ld (hl), a");
        if backward {
            self.emit_decrement_memory(source);
            self.emit_decrement_memory(destination);
        } else {
            self.emit_increment_memory(source);
            self.emit_increment_memory(destination);
        }
        self.emit_decrement_memory(length);
        self.line(&format!("    jp {loop_label}"));
    }

    fn call_return_type(&self, path: &[String]) -> Result<Type, Diagnostic> {
        let name = path_text(path);
        if self.symbols.functions.contains_key(&name) {
            let sig = self
                .symbols
                .functions
                .get(&name)
                .expect("function was checked above");
            return sig.return_type.clone().ok_or_else(|| {
                Diagnostic::new(format!("function `{name}` does not return a value"))
            });
        }
        if path.len() == 1 && self.variable_type(&name).is_some() {
            let Type::Function { return_type, .. } = self.function_pointer_type(&name)? else {
                unreachable!("function_pointer_type only returns function types")
            };
            return return_type.map(|return_type| *return_type).ok_or_else(|| {
                Diagnostic::new(format!("function pointer `{name}` does not return a value"))
            });
        }
        Err(Diagnostic::new(format!("unknown function `{name}`")))
    }

    fn call_return_width(&self, path: &[String]) -> Result<ValueWidth, Diagnostic> {
        let return_type = self.call_return_type(path)?;
        self.symbols.type_width(&return_type)
    }

    fn maybe_const_shift_count(&self, expr: &Expr) -> Result<Option<u8>, Diagnostic> {
        match self.eval_i64_with_local_constants(expr) {
            Ok(value) => self.validate_shift_count(value).map(Some),
            Err(_) => Ok(None),
        }
    }

    fn validate_shift_count(&self, value: i64) -> Result<u8, Diagnostic> {
        if !(0..=u8::MAX as i64).contains(&value) {
            return Err(Diagnostic::new(format!(
                "shift count {value} is outside supported range 0..=255"
            )));
        }
        Ok(value as u8)
    }

    fn binary_operands_are_signed(&self, left: &Expr, right: &Expr) -> Result<bool, Diagnostic> {
        Ok(
            type_is_signed(&self.symbols.resolved_type(&self.expr_type(left)?)?)
                || type_is_signed(&self.symbols.resolved_type(&self.expr_type(right)?)?),
        )
    }

    fn ensure_binary_arithmetic_operands_compatible(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.symbols.resolved_type(&self.expr_type(left)?)?;
        let right_type = self.symbols.resolved_type(&self.expr_type(right)?)?;
        if matches!(left_type, Type::Array { .. }) || matches!(right_type, Type::Array { .. }) {
            return Err(Diagnostic::new("array value cannot be used as a scalar"));
        }
        if matches!(left_type, Type::Function { .. }) || matches!(right_type, Type::Function { .. })
        {
            return Err(Diagnostic::new("type mismatch"));
        }
        if type_is_bool(&left_type) || type_is_bool(&right_type) {
            return Err(Diagnostic::new("type mismatch"));
        }
        let left_is_literal = expr_is_untyped_literal(left);
        let right_is_literal = expr_is_untyped_literal(right);
        if left_is_literal && right_is_literal {
            return Ok(());
        }

        if matches!(left_type, Type::Ptr(_)) || matches!(right_type, Type::Ptr(_)) {
            return Err(Diagnostic::new("type mismatch"));
        }

        if left_is_literal {
            let value = self.symbols.eval_i64(left)?;
            return self.symbols.validate_value_for_type(value, &right_type);
        }
        if right_is_literal {
            let value = self.symbols.eval_i64(right)?;
            return self.symbols.validate_value_for_type(value, &left_type);
        }

        if type_is_signed(&left_type) != type_is_signed(&right_type) {
            return Err(Diagnostic::new("signed/unsigned mix without cast"));
        }
        if self.symbols.type_width(&left_type)? != self.symbols.type_width(&right_type)? {
            return Err(Diagnostic::new(
                "arithmetic operands must have same width without cast",
            ));
        }
        Ok(())
    }

    fn validate_expr_assignable_to_type(
        &self,
        expr: &Expr,
        target: &Type,
    ) -> Result<(), Diagnostic> {
        self.validate_typed_literal_ranges(expr)?;
        if let Expr::Array(values) = expr {
            let Type::Array { element, len } = self.symbols.resolved_type(target)? else {
                return Err(Diagnostic::new("type mismatch"));
            };
            let len = self.symbols.array_len(&len)?;
            if values.len() as u32 > len {
                return Err(Diagnostic::new(format!(
                    "array initializer has {} values but array length is {len}",
                    values.len()
                )));
            }
            for value in values {
                self.validate_expr_assignable_to_type(value, &element)?;
            }
            return Ok(());
        }
        if let Expr::Cast { ty, .. } = expr {
            self.symbols.type_width(ty)?;
            return self.validate_type_assignable_to_type(ty, target);
        }
        if expr_is_untyped_literal(expr) {
            if let Ok(value) = self.symbols.eval_i64(expr) {
                self.symbols.validate_value_for_type(value, target)?;
            }
            return Ok(());
        }

        let source_type = self.expr_type(expr)?;
        self.validate_type_assignable_to_type(&source_type, target)
    }

    fn validate_expr_is_ptr_u8(&self, expr: &Expr) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if ty == ptr_u8_type() {
            Ok(())
        } else {
            Err(Diagnostic::new("type mismatch"))
        }
    }

    fn validate_expr_has_test_width(
        &self,
        expr: &Expr,
        width: ValueWidth,
        allow_bool: bool,
    ) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if allow_bool && type_is_bool(&ty) {
            return Ok(());
        }
        if type_is_bool(&ty) || matches!(ty, Type::Ptr(_) | Type::Function { .. }) {
            return Err(Diagnostic::new("type mismatch"));
        }
        let actual = self.symbols.type_width(&ty)?;
        if actual == width {
            return Ok(());
        }
        if let Ok(value) = self.symbols.eval_i64(expr) {
            self.symbols
                .wrap_value_for_type(value, &width_unsigned_type(width))?;
            return Ok(());
        }
        if actual < width {
            return Err(Diagnostic::new("widening without cast"));
        }
        if actual > width {
            return Err(Diagnostic::new("narrowing without cast"));
        }
        Ok(())
    }

    fn validate_type_assignable_to_type(
        &self,
        source: &Type,
        target: &Type,
    ) -> Result<(), Diagnostic> {
        let source_type = self.symbols.resolved_type(source)?;
        let target_type = self.symbols.resolved_type(target)?;
        if source_type == target_type {
            return Ok(());
        }
        if let (
            Type::Array {
                element: source_element,
                len: source_len,
            },
            Type::Array {
                element: target_element,
                len: target_len,
            },
        ) = (&source_type, &target_type)
        {
            if self.symbols.array_len(source_len)? != self.symbols.array_len(target_len)? {
                return Err(Diagnostic::new("type mismatch"));
            }
            return self.validate_type_assignable_to_type(source_element, target_element);
        }
        if matches!(source_type, Type::Array { .. }) || matches!(target_type, Type::Array { .. }) {
            return Err(Diagnostic::new("type mismatch"));
        }
        if type_is_bool(&source_type) || type_is_bool(&target_type) {
            return Err(Diagnostic::new("type mismatch"));
        }
        if matches!(source_type, Type::Ptr(_) | Type::Function { .. })
            || matches!(target_type, Type::Ptr(_) | Type::Function { .. })
        {
            return Err(Diagnostic::new("type mismatch"));
        }

        let source_width = self.symbols.type_width(&source_type)?;
        let target_width = self.symbols.type_width(&target_type)?;
        if source_width < target_width {
            return Err(Diagnostic::new("widening without cast"));
        }
        if source_width > target_width {
            return Err(Diagnostic::new("narrowing without cast"));
        }
        if type_is_signed(&source_type) != type_is_signed(&target_type) {
            return Err(Diagnostic::new("signed/unsigned mix without cast"));
        }
        Err(Diagnostic::new("type mismatch"))
    }

    fn validate_typed_literal_ranges(&self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::TypedInt(value, ty) => self.symbols.validate_value_for_type(*value, ty),
            Expr::Unary {
                op: UnaryOp::Neg,
                expr,
            } => {
                if let Expr::TypedInt(value, ty) = expr.as_ref() {
                    let value = value.checked_neg().ok_or_else(|| {
                        Diagnostic::new(format!("value -{value} is outside i24 range"))
                    })?;
                    self.symbols.validate_value_for_type(value, ty)
                } else {
                    self.validate_typed_literal_ranges(expr)
                }
            }
            Expr::Unary { expr, .. }
            | Expr::Cast { expr, .. }
            | Expr::Deref(expr)
            | Expr::BankedPointer { pointer: expr, .. } => self.validate_typed_literal_ranges(expr),
            Expr::Binary { left, right, .. } => {
                self.validate_typed_literal_ranges(left)?;
                self.validate_typed_literal_ranges(right)
            }
            Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
                self.validate_typed_literal_ranges(index)
            }
            Expr::Access(path) | Expr::AddressOfAccess(path) => {
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        self.validate_typed_literal_ranges(index)?;
                    }
                }
                Ok(())
            }
            Expr::Array(values) => {
                for value in values {
                    self.validate_typed_literal_ranges(value)?;
                }
                Ok(())
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    self.validate_typed_literal_ranges(value)?;
                }
                Ok(())
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.validate_typed_literal_ranges(arg)?;
                }
                Ok(())
            }
            Expr::Int(_)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::String(_)
            | Expr::Ident(_)
            | Expr::In(_)
            | Expr::Field { .. }
            | Expr::AddressOf(_)
            | Expr::AddressOfField { .. } => Ok(()),
        }
    }

    fn validate_expr_arithmetic_compatibility(&self, expr: &Expr) -> Result<(), Diagnostic> {
        self.validate_typed_literal_ranges(expr)?;
        match expr {
            Expr::Binary { left, op, right } => {
                self.validate_expr_arithmetic_compatibility(left)?;
                self.validate_expr_arithmetic_compatibility(right)?;
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    self.ensure_expr_is_bool(left, "logical operand")?;
                    self.ensure_expr_is_bool(right, "logical operand")?;
                } else if is_comparison(*op) {
                    self.ensure_comparison_operands_compatible(left, *op, right)?;
                } else if matches!(op, BinaryOp::Add | BinaryOp::Sub)
                    && (self.pointer_pointee_size(left)?.is_some()
                        || self.pointer_pointee_size(right)?.is_some())
                {
                    self.ensure_pointer_arithmetic_expr_compatible(left, *op, right)?;
                } else if matches!(op, BinaryOp::Shl | BinaryOp::Shr) {
                    self.ensure_shift_operands_compatible(left, right)?;
                } else if matches!(
                    op,
                    BinaryOp::Add
                        | BinaryOp::Sub
                        | BinaryOp::Mul
                        | BinaryOp::Div
                        | BinaryOp::Mod
                        | BinaryOp::BitAnd
                        | BinaryOp::BitOr
                        | BinaryOp::BitXor
                ) {
                    self.ensure_binary_arithmetic_operands_compatible(left, right)?;
                }
            }
            Expr::Unary { expr, op } => {
                self.validate_expr_arithmetic_compatibility(expr)?;
                match op {
                    UnaryOp::Not => self.ensure_expr_is_bool(expr, "logical operand")?,
                    UnaryOp::Neg | UnaryOp::BitNot => {
                        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
                        validate_integer_unary_operand_type(&ty)?;
                    }
                }
            }
            Expr::Cast { expr, ty } => {
                self.validate_expr_arithmetic_compatibility(expr)?;
                self.validate_cast(expr, ty)?;
            }
            Expr::Deref(expr) | Expr::BankedPointer { pointer: expr, .. } => {
                self.validate_expr_arithmetic_compatibility(expr)?;
            }
            Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
                self.validate_expr_arithmetic_compatibility(index)?;
            }
            Expr::Access(path) | Expr::AddressOfAccess(path) => {
                for segment in &path.segments {
                    if let AccessSegment::Index(index) = segment {
                        self.validate_expr_arithmetic_compatibility(index)?;
                    }
                }
            }
            Expr::Array(values) => {
                for value in values {
                    self.validate_expr_arithmetic_compatibility(value)?;
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    self.validate_expr_arithmetic_compatibility(value)?;
                }
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.validate_expr_arithmetic_compatibility(arg)?;
                }
            }
            Expr::Int(_)
            | Expr::TypedInt(_, _)
            | Expr::Bool(_)
            | Expr::Char(_)
            | Expr::String(_)
            | Expr::Ident(_)
            | Expr::In(_)
            | Expr::Field { .. }
            | Expr::AddressOf(_)
            | Expr::AddressOfField { .. } => {}
        }
        Ok(())
    }

    fn ensure_shift_operands_compatible(
        &self,
        left: &Expr,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.symbols.resolved_type(&self.expr_type(left)?)?;
        validate_shift_operand_type(&left_type)?;
        self.ensure_shift_count_compatible(right)
    }

    fn ensure_shift_count_compatible(&self, count: &Expr) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(count)?)?;
        validate_shift_count_integer_type(&ty)?;

        if let Ok(value) = self.eval_i64_with_local_constants(count) {
            self.validate_shift_count(value)?;
            return Ok(());
        }

        validate_runtime_shift_count_type(&ty)
    }

    fn ensure_pointer_arithmetic_expr_compatible(
        &self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_scale = self.pointer_pointee_size(left)?;
        let right_scale = self.pointer_pointee_size(right)?;
        match (op, left_scale, right_scale) {
            (BinaryOp::Add, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer arithmetic requires exactly one pointer operand",
            )),
            (BinaryOp::Add, Some(_), None) => self.ensure_pointer_offset_expr(right),
            (BinaryOp::Add, None, Some(_)) => self.ensure_pointer_offset_expr(left),
            (BinaryOp::Sub, Some(_), Some(_)) => Err(Diagnostic::new(
                "pointer subtraction between two pointers is not supported",
            )),
            (BinaryOp::Sub, Some(_), None) => self.ensure_pointer_offset_expr(right),
            (BinaryOp::Sub, None, Some(_)) => Err(Diagnostic::new(
                "cannot subtract a pointer from a non-pointer value",
            )),
            _ => Ok(()),
        }
    }

    fn ensure_comparison_operands_compatible(
        &self,
        left: &Expr,
        op: BinaryOp,
        right: &Expr,
    ) -> Result<(), Diagnostic> {
        let left_type = self.symbols.resolved_type(&self.expr_type(left)?)?;
        let right_type = self.symbols.resolved_type(&self.expr_type(right)?)?;
        if let Some(name) = struct_scalar_type(&left_type, &self.symbols.structs) {
            return Err(Diagnostic::new(format!(
                "struct `{name}` cannot be used as a scalar value"
            )));
        }
        if let Some(name) = struct_scalar_type(&right_type, &self.symbols.structs) {
            return Err(Diagnostic::new(format!(
                "struct `{name}` cannot be used as a scalar value"
            )));
        }
        if !type_is_bool(&left_type)
            && !type_is_bool(&right_type)
            && !matches!(left_type, Type::Ptr(_))
            && !matches!(right_type, Type::Ptr(_))
        {
            if expr_is_untyped_literal(left) && expr_is_untyped_literal(right) {
                return Ok(());
            }
            if expr_is_untyped_literal(left) {
                let value = self.symbols.eval_i64(left)?;
                return self.symbols.validate_value_for_type(value, &right_type);
            }
            if expr_is_untyped_literal(right) {
                let value = self.symbols.eval_i64(right)?;
                return self.symbols.validate_value_for_type(value, &left_type);
            }
        }
        validate_comparison_types(&left_type, op, &right_type, || {
            if expr_is_untyped_literal(left) || expr_is_untyped_literal(right) {
                None
            } else {
                Some((
                    self.symbols.type_width(&left_type).ok()?,
                    self.symbols.type_width(&right_type).ok()?,
                ))
            }
        })
    }

    fn ensure_expr_is_bool(&self, expr: &Expr, context: &str) -> Result<(), Diagnostic> {
        let ty = self.symbols.resolved_type(&self.expr_type(expr)?)?;
        if type_is_bool(&ty) {
            Ok(())
        } else {
            Err(Diagnostic::new(format!("{context} must be bool")))
        }
    }

    fn current_return_type(&self) -> &Type {
        self.return_type_stack
            .last()
            .and_then(|ty| ty.as_ref())
            .expect("function return type exists during value return emission")
    }

    fn current_function_requires_return_value(&self) -> bool {
        *self
            .return_value_stack
            .last()
            .expect("function return kind exists during emission")
    }

    fn current_function_returns_two_values(&self) -> bool {
        self.second_return_type_stack
            .last()
            .is_some_and(Option::is_some)
    }

    fn current_function_name(&self) -> &str {
        self.function_name_stack
            .last()
            .expect("function name exists during emission")
    }

    fn current_function_uses_frame(&self) -> bool {
        self.function_frame_stack
            .last()
            .copied()
            .expect("function frame state exists during emission")
    }

    fn current_function_is_interrupt(&self) -> bool {
        self.function_interrupt_stack
            .last()
            .copied()
            .expect("function interrupt state exists during emission")
    }

    fn current_function_is_naked(&self) -> bool {
        self.function_naked_stack
            .last()
            .copied()
            .expect("function naked state exists during emission")
    }

    fn port(&self, name: &str) -> Result<u8, Diagnostic> {
        self.symbols
            .ports
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new(format!("unknown port `{name}`")))
    }

    fn variable(&self, name: &str) -> Result<Variable, Diagnostic> {
        self.variable_opt(name)
            .ok_or_else(|| Diagnostic::new(format!("unknown variable `{name}`")))
    }

    fn dotted_variable(&self, base: &str, field: &str) -> Option<Variable> {
        self.variable_opt(&format!("{base}.{field}"))
    }

    fn variable_opt(&self, name: &str) -> Option<Variable> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .or_else(|| self.symbols.globals.get(name).copied())
    }

    fn variable_type(&self, name: &str) -> Option<&Type> {
        self.scope_types
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.symbols.global_types.get(name))
    }

    fn named_value_type(&self, name: &str) -> Option<&Type> {
        self.variable_type(name)
            .or_else(|| self.symbols.constant_types.get(name))
    }

    fn embed_property_type(&self, name: &str) -> Option<Type> {
        self.symbols.embed_property_value(name)?;
        let (_, property) = name.rsplit_once('.')?;
        match property {
            "ptr" | "end" => Some(Type::Ptr(Box::new(Type::Named("u8".to_owned())))),
            "len" => Some(Type::Named("u24".to_owned())),
            _ => None,
        }
    }

    fn canonical_access_path(&self, path: &AccessPath) -> AccessPath {
        if self.named_value_type(&path.root).is_some() {
            return path.clone();
        }

        let mut candidate = path.root.clone();
        let mut best = None;
        for (index, segment) in path.segments.iter().enumerate() {
            let AccessSegment::Field(field) = segment else {
                break;
            };
            candidate.push('.');
            candidate.push_str(field);
            if self.named_value_type(&candidate).is_some()
                || self.symbols.embed_property_value(&candidate).is_some()
            {
                best = Some((candidate.clone(), index + 1));
            }
            if let Some((_, original)) = candidate.split_once('.')
                && (self.named_value_type(original).is_some()
                    || self.symbols.embed_property_value(original).is_some())
            {
                best = Some((original.to_owned(), index + 1));
            }
        }

        if let Some((root, consumed)) = best {
            AccessPath {
                root,
                segments: path.segments[consumed..].to_vec(),
            }
        } else {
            path.clone()
        }
    }

    fn name_in_current_function(&self, name: &str) -> bool {
        self.scope_types
            .iter()
            .any(|scope| scope.contains_key(name))
            || self.symbols.global_types.contains_key(name)
            || self.symbols.constant_types.contains_key(name)
            || self.symbols.functions.contains_key(name)
    }

    fn current_scope_mut(&mut self) -> &mut HashMap<String, Variable> {
        self.scopes
            .last_mut()
            .expect("function scope exists during statement emission")
    }

    fn current_scope_types_mut(&mut self) -> &mut HashMap<String, Type> {
        self.scope_types
            .last_mut()
            .expect("function type scope exists during statement emission")
    }

    fn current_local_constants_mut(&mut self) -> &mut HashMap<String, LocalConstant> {
        self.local_constants
            .last_mut()
            .expect("function local constant scope exists during statement emission")
    }

    fn current_readonly_pointer_aliases_mut(&mut self) -> &mut HashMap<String, u32> {
        self.readonly_pointer_aliases
            .last_mut()
            .expect("function read-only pointer alias scope exists during statement emission")
    }

    fn local_constant(&self, name: &str) -> Option<&LocalConstant> {
        for index in (0..self.local_constants.len()).rev() {
            if let Some(constant) = self.local_constants[index].get(name) {
                return Some(constant);
            }
            if self
                .scope_types
                .get(index)
                .is_some_and(|scope| scope.contains_key(name))
            {
                return None;
            }
        }
        None
    }

    fn readonly_pointer_alias(&self, name: &str) -> Option<u32> {
        for index in (0..self.readonly_pointer_aliases.len()).rev() {
            if let Some(addr) = self.readonly_pointer_aliases[index].get(name) {
                return Some(*addr);
            }
            if self
                .scope_types
                .get(index)
                .is_some_and(|scope| scope.contains_key(name))
            {
                return None;
            }
        }
        None
    }

    fn current_function_assigns(&self, name: &str) -> bool {
        self.assigned_names_stack
            .last()
            .is_some_and(|names| names.contains(name))
    }

    fn current_function_requires_storage(&self, name: &str) -> bool {
        self.storage_required_names_stack
            .last()
            .is_some_and(|names| names.contains(name))
    }

    fn can_elide_constant_local_storage(
        &self,
        name: &str,
        ty: &Type,
        value: &Expr,
    ) -> Result<bool, Diagnostic> {
        if self
            .function_local_plans
            .last()
            .is_some_and(|locals| locals.contains_key(name))
        {
            return Ok(false);
        }
        if self.current_function_assigns(name)
            || self.current_function_requires_storage(name)
            || !self.local_constant_supported_type(ty)
        {
            return Ok(false);
        }
        self.validate_expr_arithmetic_compatibility(value)?;
        self.validate_expr_assignable_to_type(value, ty)?;
        let width = self.symbols.type_width(ty)?;
        let Ok(value) = self.eval_i64_with_local_constants(value) else {
            return Ok(false);
        };
        Ok(self.value_for_type(value, ty, width).is_ok())
    }

    fn record_local_constant(&mut self, name: &str, ty: &Type, value: &Expr) {
        if self.current_function_assigns(name) {
            self.current_local_constants_mut().remove(name);
            return;
        }
        if !self.local_constant_supported_type(ty) {
            self.current_local_constants_mut().remove(name);
            return;
        }
        let Ok(width) = self.symbols.type_width(ty) else {
            return;
        };
        let Ok(value) = self.eval_i64_with_local_constants(value) else {
            self.current_local_constants_mut().remove(name);
            return;
        };
        if self.value_for_type(value, ty, width).is_ok() {
            self.current_local_constants_mut().insert(
                name.to_owned(),
                LocalConstant {
                    value,
                    ty: ty.clone(),
                },
            );
        }
    }

    fn local_constant_supported_type(&self, ty: &Type) -> bool {
        match self.symbols.resolved_type(ty) {
            Ok(Type::Ptr(_)) => true,
            Ok(Type::Named(name)) => matches!(
                name.as_str(),
                "u8" | "i8" | "bool" | "u16" | "i16" | "u24" | "i24" | "ptr"
            ),
            _ => false,
        }
    }

    fn invalidate_local_constant(&mut self, name: &str) {
        for scope in self.local_constants.iter_mut().rev() {
            if scope.remove(name).is_some() {
                return;
            }
        }
    }

    fn invalidate_all_local_constants(&mut self) {
        for scope in &mut self.local_constants {
            scope.clear();
        }
    }

    fn record_readonly_pointer_alias(&mut self, name: &str, value: &Expr) {
        let Some(addr) = self.readonly_write_addr(value) else {
            self.current_readonly_pointer_aliases_mut().remove(name);
            return;
        };
        if self.readonly_embed_name_for_addr(addr).is_some()
            || self.readonly_string_literal_for_addr(addr).is_some()
        {
            self.current_readonly_pointer_aliases_mut()
                .insert(name.to_owned(), addr);
        } else {
            self.current_readonly_pointer_aliases_mut().remove(name);
        }
    }

    fn readonly_embed_name_for_addr(&self, addr: u32) -> Option<&str> {
        let addr = u64::from(addr);
        for (name, embed) in &self.symbols.embeds {
            let Some(len) = embed.variable.len else {
                continue;
            };
            let start = u64::from(embed.variable.addr);
            let end = start + u64::from(len);
            if addr >= start && addr < end {
                return Some(name.as_str());
            }
        }
        None
    }

    fn readonly_string_literal_for_addr(&self, addr: u32) -> Option<&str> {
        self.readonly_string_literal_for_range(u64::from(addr), u64::from(addr) + 1)
    }

    fn readonly_string_literal_for_range(&self, start: u64, end: u64) -> Option<&str> {
        for (value, variable) in self
            .string_literals
            .iter()
            .chain(self.symbols.string_literals.iter())
        {
            let Some(len) = variable.len else {
                continue;
            };
            if len == 0 {
                continue;
            }
            let literal_start = u64::from(variable.addr);
            let literal_end = literal_start + u64::from(len);
            if start < literal_end && end > literal_start {
                return Some(value.as_str());
            }
        }
        None
    }

    fn invalidate_readonly_pointer_alias(&mut self, name: &str) {
        for scope in self.readonly_pointer_aliases.iter_mut().rev() {
            if scope.remove(name).is_some() {
                return;
            }
        }
    }

    fn next_label(&mut self, prefix: &str) -> String {
        let label = format!(".L_{prefix}_{}", self.label_counter);
        self.label_counter += 1;
        label
    }

    fn line(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
    }
}

fn trunc_div_or_zero(left: i64, right: i64) -> i64 {
    if right == 0 {
        0
    } else {
        ((left as i128) / (right as i128)) as i64
    }
}

fn trunc_mod_or_zero(left: i64, right: i64) -> i64 {
    if right == 0 {
        0
    } else {
        ((left as i128) % (right as i128)) as i64
    }
}

fn alloc_from_cursor(cursor: &mut u32, align: u32, size: u32) -> Result<Variable, Diagnostic> {
    if align > 1 {
        let mask = align - 1;
        *cursor = cursor
            .checked_add(mask)
            .map(|addr| addr & !mask)
            .ok_or_else(|| Diagnostic::new("section alignment exceeds 24-bit address space"))?;
    }
    let variable = Variable {
        addr: *cursor,
        size,
        element_size: Some(u32::from(ValueWidth::U8.bytes())),
        len: Some(size),
    };
    *cursor = cursor
        .checked_add(size)
        .ok_or_else(|| Diagnostic::new("section allocation exceeds 24-bit address space"))?;
    if *cursor > Address24::MAX + 1 {
        return Err(Diagnostic::new(
            "section allocation exceeds 24-bit address space",
        ));
    }
    Ok(variable)
}

fn section_cursor<'a>(cursors: &'a mut [(String, u32)], section: &str) -> &'a mut u32 {
    let index = cursors
        .iter()
        .position(|(name, _)| name == section)
        .expect("section cursor exists");
    &mut cursors[index].1
}

fn recursive_call_edges(
    program: &Program,
    functions: &HashMap<String, FunctionSig>,
) -> HashSet<(String, String)> {
    let graph = function_call_graph(program, functions);
    let mut edges = HashSet::new();
    for (caller, callees) in &graph {
        for callee in callees {
            if function_reaches(callee, caller, &graph) {
                edges.insert((caller.clone(), callee.clone()));
            }
        }
    }
    edges
}

fn function_call_graph(
    program: &Program,
    functions: &HashMap<String, FunctionSig>,
) -> HashMap<String, Vec<String>> {
    let mut graph = HashMap::new();
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        let mut calls = Vec::new();
        collect_stmt_calls(&function.body, &mut calls);
        collect_stmt_function_references(&function.body, &mut calls);
        calls.retain(|name| functions.contains_key(name));
        graph.insert(function.name.clone(), calls);
    }
    graph
}

fn function_reaches(start: &str, target: &str, graph: &HashMap<String, Vec<String>>) -> bool {
    let mut stack = vec![start.to_owned()];
    let mut visited = HashSet::new();
    while let Some(function) = stack.pop() {
        if !visited.insert(function.clone()) {
            continue;
        }
        if function == target {
            return true;
        }
        if let Some(calls) = graph.get(&function) {
            stack.extend(calls.iter().cloned());
        }
    }
    false
}

fn validate_all_function_calls(
    program: &Program,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        validate_stmt_calls(&function.body, functions)?;
    }
    Ok(())
}

fn validate_all_function_bodies(
    program: &Program,
    symbols: Symbols,
    options: AssemblyOptions,
    recursive_call_edges: HashSet<(String, String)>,
    tail_call_edges: HashSet<(String, String)>,
) -> Result<(), Diagnostic> {
    let mut emitter = Emitter::new(
        symbols,
        options.clone(),
        recursive_call_edges,
        tail_call_edges,
        None,
    );
    emitter.disable_dead_code_elimination();
    if let Some(main) = program.main_function() {
        emitter.emit_function(main)?;
    }
    for declaration in &program.declarations {
        let Declaration::Function(function) = declaration else {
            continue;
        };
        if function.name == "main" {
            continue;
        }
        emitter.emit_function(function)?;
    }
    Ok(())
}

fn validate_stmt_calls(
    stmts: &[Stmt],
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::LetTwo { value, .. } => {
                validate_expr_calls(value, functions)?
            }
            Stmt::Assign { target, value, .. } => {
                validate_place_calls(target, functions)?;
                validate_expr_calls(value, functions)?;
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                validate_expr_calls(condition, functions)?;
                validate_stmt_calls(then_body, functions)?;
                validate_stmt_calls(else_body, functions)?;
            }
            Stmt::While { condition, body } => {
                validate_expr_calls(condition, functions)?;
                validate_stmt_calls(body, functions)?;
            }
            Stmt::Loop { body } => validate_stmt_calls(body, functions)?,
            Stmt::Return(Some(expr)) | Stmt::Expr(expr) => validate_expr_calls(expr, functions)?,
            Stmt::ReturnTwo { first, second } => {
                validate_expr_calls(first, functions)?;
                validate_expr_calls(second, functions)?;
            }
            Stmt::Out { value, .. } => validate_expr_calls(value, functions)?,
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => {}
        }
    }
    Ok(())
}

fn collect_stmt_call_diagnostics(
    stmts: &[Stmt],
    spans: &[crate::ast::StmtSpan],
    functions: &HashMap<String, FunctionSig>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (index, stmt) in stmts.iter().enumerate() {
        let result = match stmt {
            Stmt::Let { value, .. } | Stmt::LetTwo { value, .. } => {
                validate_expr_calls(value, functions)
            }
            Stmt::Assign { target, value, .. } => validate_place_calls(target, functions)
                .and_then(|_| validate_expr_calls(value, functions)),
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                let result = validate_expr_calls(condition, functions);
                let children = spans
                    .get(index)
                    .map_or(&[][..], |span| span.children.as_slice());
                collect_stmt_call_diagnostics(
                    then_body,
                    &children[..children.len().min(then_body.len())],
                    functions,
                    diagnostics,
                );
                collect_stmt_call_diagnostics(
                    else_body,
                    &children[children.len().min(then_body.len())..],
                    functions,
                    diagnostics,
                );
                result
            }
            Stmt::While { condition, body } => {
                let result = validate_expr_calls(condition, functions);
                let children = spans
                    .get(index)
                    .map_or(&[][..], |span| span.children.as_slice());
                collect_stmt_call_diagnostics(body, children, functions, diagnostics);
                result
            }
            Stmt::Loop { body } => {
                let children = spans
                    .get(index)
                    .map_or(&[][..], |span| span.children.as_slice());
                collect_stmt_call_diagnostics(body, children, functions, diagnostics);
                Ok(())
            }
            Stmt::Return(Some(expr)) | Stmt::Expr(expr) => validate_expr_calls(expr, functions),
            Stmt::ReturnTwo { first, second } => validate_expr_calls(first, functions)
                .and_then(|_| validate_expr_calls(second, functions)),
            Stmt::Out { value, .. } => validate_expr_calls(value, functions),
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => Ok(()),
        };
        if let Err(error) = result {
            let error = spans
                .get(index)
                .map(|span| locate_statement_diagnostic(span, error.clone()))
                .unwrap_or(error);
            diagnostics.push(error);
        }
    }
}

fn locate_statement_diagnostic(statement: &crate::ast::StmtSpan, error: Diagnostic) -> Diagnostic {
    let quoted = error
        .message
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>();
    statement
        .references
        .iter()
        .filter(|reference| quoted.iter().any(|token| reference.text == *token))
        .min_by_key(|reference| {
            (
                reference
                    .span
                    .end
                    .line
                    .saturating_sub(reference.span.start.line),
                reference
                    .span
                    .end
                    .column
                    .saturating_sub(reference.span.start.column),
            )
        })
        .map(|reference| error.clone().with_span_if_missing(reference.span.clone()))
        .unwrap_or_else(|| error.with_span_if_missing(statement.span.clone()))
}

fn validate_place_calls(
    place: &Place,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => validate_expr_calls(index, functions),
        Place::Access(path) => validate_access_calls(path, functions),
        Place::Ident(_) | Place::Field { .. } => Ok(()),
    }
}

fn validate_expr_calls(
    expr: &Expr,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Array(values) => {
            for value in values {
                validate_expr_calls(value, functions)?;
            }
        }
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } | Expr::Deref(index) => {
            validate_expr_calls(index, functions)?;
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            validate_access_calls(path, functions)?;
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                validate_expr_calls(value, functions)?;
            }
        }
        Expr::Call { path, args } => {
            let name = path_text(path);
            validate_call_signature(&name, args.len(), functions)?;
            for arg in args {
                validate_expr_calls(arg, functions)?;
            }
        }
        Expr::Unary { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::BankedPointer { pointer: expr, .. } => validate_expr_calls(expr, functions)?,
        Expr::Binary { left, right, .. } => {
            validate_expr_calls(left, functions)?;
            validate_expr_calls(right, functions)?;
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
    Ok(())
}

fn validate_access_calls(
    path: &AccessPath,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            validate_expr_calls(index, functions)?;
        }
    }
    Ok(())
}

fn validate_call_signature(
    name: &str,
    arity: usize,
    functions: &HashMap<String, FunctionSig>,
) -> Result<(), Diagnostic> {
    if let Some(expected) = builtin_function_arity(name) {
        if expected != arity {
            return Err(Diagnostic::new(builtin_arity_error(name, arity)));
        }
        return Ok(());
    }
    let Some(sig) = functions.get(name) else {
        // A single-name call may be a call through a local or global
        // `ptr<fn(...)>` binding. The emitter validates its typed signature
        // once the binding scope is available.
        if !name.contains('.') {
            return Ok(());
        }
        return Err(Diagnostic::new(format!("unknown function `{name}`")));
    };
    if sig.arity != arity {
        return Err(Diagnostic::new(format!(
            "function `{name}` expects {} arguments but got {arity}",
            sig.arity
        )));
    }
    Ok(())
}

fn is_legacy_memory_intrinsic(name: &str) -> bool {
    matches!(
        name,
        "mem.peek8"
            | "ezra.mem.peek8"
            | "mem.poke8"
            | "ezra.mem.poke8"
            | "mem.memcpy"
            | "ezra.mem.memcpy"
            | "mem.memset"
            | "ezra.mem.memset"
    )
}

fn builtin_function_arity(name: &str) -> Option<usize> {
    if let Some(descriptor) = CATALOG.lookup(name) {
        return Some(descriptor.argument_count);
    }
    match name {
        "test.pass" | "ezra.test.pass" => Some(0),
        "test.fail" | "ezra.test.fail" => Some(1),
        "test.assert_eq_u8" | "ezra.test.assert_eq_u8" => Some(3),
        "test.assert_eq_u16" | "ezra.test.assert_eq_u16" => Some(3),
        "test.assert_eq_u24" | "ezra.test.assert_eq_u24" => Some(3),
        "debug.char" | "ezra.debug.char" => Some(1),
        "debug.str" | "ezra.debug.str" => Some(1),
        "debug.hex_u8" | "ezra.debug.hex_u8" => Some(1),
        "debug.hex_u16" | "ezra.debug.hex_u16" => Some(1),
        "debug.hex_u24" | "ezra.debug.hex_u24" => Some(1),
        "mem.poke8" | "ezra.mem.poke8" => Some(2),
        "mem.peek8" | "ezra.mem.peek8" => Some(1),
        "mem.memcpy" | "ezra.mem.memcpy" => Some(3),
        "mem.memset" | "ezra.mem.memset" => Some(3),
        _ => None,
    }
}

fn builtin_arity_error(name: &str, actual: usize) -> String {
    if let Some(descriptor) = CATALOG.lookup(name) {
        return format!(
            "intrinsic `{}` expects {} arguments, got {actual}",
            descriptor.canonical_name, descriptor.argument_count
        );
    }
    match name.strip_prefix("ezra.").unwrap_or(name) {
        "test.pass" => "test.pass requires no arguments".to_owned(),
        "test.fail" => "test.fail requires one argument".to_owned(),
        "test.assert_eq_u8" => "test.assert_eq_u8 requires three arguments".to_owned(),
        "test.assert_eq_u16" => "test.assert_eq_u16 requires three arguments".to_owned(),
        "test.assert_eq_u24" => "test.assert_eq_u24 requires three arguments".to_owned(),
        "debug.char" => "debug.char requires one argument".to_owned(),
        "debug.str" => "debug.str requires one argument".to_owned(),
        "debug.hex_u8" => "debug.hex_u8 requires one argument".to_owned(),
        "debug.hex_u16" => "debug.hex_u16 requires one argument".to_owned(),
        "debug.hex_u24" => "debug.hex_u24 requires one argument".to_owned(),
        "mem.poke8" => "mem.poke8 requires two arguments".to_owned(),
        "mem.peek8" => "mem.peek8 requires one argument".to_owned(),
        "mem.memcpy" => "mem.memcpy requires three arguments".to_owned(),
        "mem.memset" => "mem.memset requires three arguments".to_owned(),
        builtin => format!("{builtin} has invalid argument count"),
    }
}

fn program_contains_function_pointer_locals(
    program: &Program,
    options: &AssemblyOptions,
) -> Result<bool, Diagnostic> {
    let symbols = Symbols::from_program(program, options.clone(), None)?;
    for declaration in &program.declarations {
        let Declaration::Function(function) = unwrapped_declaration(declaration) else {
            continue;
        };
        if statements_contain_function_pointer_local(&function.body, &symbols)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn statements_contain_function_pointer_local(
    statements: &[Stmt],
    symbols: &Symbols,
) -> Result<bool, Diagnostic> {
    for statement in statements {
        match statement {
            Stmt::Let { ty, .. } => {
                let resolved = symbols.resolved_type(ty)?;
                if matches!(resolved, Type::Ptr(inner) if matches!(inner.as_ref(), Type::Function { .. }))
                {
                    return Ok(true);
                }
            }
            Stmt::LetTwo {
                first_ty,
                second_ty,
                ..
            } => {
                for ty in [first_ty, second_ty] {
                    let resolved = symbols.resolved_type(ty)?;
                    if matches!(resolved, Type::Ptr(inner) if matches!(inner.as_ref(), Type::Function { .. }))
                    {
                        return Ok(true);
                    }
                }
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                if statements_contain_function_pointer_local(then_body, symbols)?
                    || statements_contain_function_pointer_local(else_body, symbols)?
                {
                    return Ok(true);
                }
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                if statements_contain_function_pointer_local(body, symbols)? {
                    return Ok(true);
                }
            }
            _ => {}
        }
    }
    Ok(false)
}

fn reachable_function_names(program: &Program, symbols: &Symbols) -> HashSet<String> {
    let mut graph = HashMap::new();
    let mut seeds = Vec::new();
    let mut has_extern_assembly = false;
    for declaration in &program.declarations {
        match unwrapped_declaration(declaration) {
            Declaration::Function(function) => {
                let mut calls = Vec::new();
                collect_reachable_stmt_calls(&function.body, &mut calls, symbols);
                collect_stmt_function_references(&function.body, &mut calls);
                calls.retain(|name| symbols.functions.contains_key(name));
                graph.insert(function.name.clone(), calls);
                if function.name == "main"
                    || has_attr(function, "extern")
                    || has_attr(function, "naked")
                    || has_attr(function, "interrupt")
                    || declaration_is_banked(declaration)
                {
                    seeds.push(function.name.clone());
                }
            }
            Declaration::Global(global) => {
                let mut references = Vec::new();
                collect_expr_function_references(&global.value, &mut references);
                seeds.extend(
                    references
                        .into_iter()
                        .filter(|name| symbols.functions.contains_key(name)),
                );
            }
            Declaration::ExternAsmFunction(_) => has_extern_assembly = true,
            _ => {}
        }
    }

    let mut reachable = HashSet::new();
    let mut stack = seeds;
    while let Some(name) = stack.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        if let Some(calls) = graph.get(&name) {
            stack.extend(calls.iter().cloned());
        }
    }
    if has_extern_assembly {
        return graph.keys().cloned().collect();
    }

    // Keep address-taken functions even when the reference is stored in a
    // static initializer or passes through an optimizer-removed binding.
    for declaration in &program.declarations {
        let mut references = Vec::new();
        match unwrapped_declaration(declaration) {
            Declaration::Global(global) => {
                collect_expr_function_references(&global.value, &mut references)
            }
            Declaration::Function(function) => {
                collect_stmt_function_references(&function.body, &mut references)
            }
            _ => {}
        }
        reachable.extend(
            references
                .into_iter()
                .filter(|name| symbols.functions.contains_key(name)),
        );
    }
    reachable
}

fn function_declaration<'a>(program: &'a Program, name: &str) -> Option<&'a Function> {
    program.declarations.iter().find_map(|declaration| {
        let Declaration::Function(function) = unwrapped_declaration(declaration) else {
            return None;
        };
        (function.name == name).then_some(function)
    })
}

fn function_contains_inline_asm(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Asm { .. } => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => function_contains_inline_asm(then_body) || function_contains_inline_asm(else_body),
        Stmt::While { body, .. } | Stmt::Loop { body } => function_contains_inline_asm(body),
        _ => false,
    })
}

#[derive(Default)]
struct StaticReferences {
    names: HashSet<String>,
    string_literals: HashSet<String>,
}

fn static_liveness(program: &Program, emitted_functions: &HashSet<String>) -> StaticLiveness {
    let mut all_constants = HashSet::new();
    let mut all_globals = HashSet::new();
    let mut all_embeds = HashSet::new();
    for declaration in &program.declarations {
        match unwrapped_declaration(declaration) {
            Declaration::Const(decl) => {
                all_constants.insert(decl.name.clone());
            }
            Declaration::Global(decl) => {
                all_globals.insert(decl.name.clone());
            }
            Declaration::Embed(decl) => {
                all_embeds.insert(decl.name.clone());
            }
            _ => {}
        }
    }

    let mut liveness = StaticLiveness::default();
    for declaration in &program.declarations {
        let declaration_is_root = declaration_is_banked(declaration);
        match unwrapped_declaration(declaration) {
            Declaration::Const(decl) if declaration_is_root => {
                liveness.constants.insert(decl.name.clone());
            }
            Declaration::Global(decl) if declaration_is_root => {
                liveness.globals.insert(decl.name.clone());
            }
            Declaration::Embed(decl) if declaration_is_root => {
                liveness.embeds.insert(decl.name.clone());
            }
            _ => {}
        }
    }

    let mut opaque_assembly = program.declarations.iter().any(|declaration| {
        matches!(
            unwrapped_declaration(declaration),
            Declaration::ExternAsmFunction(_)
        )
    });
    for declaration in &program.declarations {
        let Declaration::Function(function) = unwrapped_declaration(declaration) else {
            continue;
        };
        if !emitted_functions.contains(&function.name) {
            continue;
        }
        opaque_assembly |= function_contains_inline_asm(&function.body);
        if !opaque_assembly {
            let mut references = StaticReferences::default();
            collect_stmt_static_references(&function.body, &mut references);
            apply_static_references(
                &mut liveness,
                &references,
                &all_constants,
                &all_globals,
                &all_embeds,
            );
        }
    }

    if opaque_assembly {
        liveness.constants = all_constants;
        liveness.globals = all_globals;
        liveness.embeds = all_embeds;
        let mut references = StaticReferences::default();
        for declaration in &program.declarations {
            match unwrapped_declaration(declaration) {
                Declaration::Const(decl) => {
                    collect_expr_static_references(&decl.value, &mut references)
                }
                Declaration::Global(decl) => {
                    collect_expr_static_references(&decl.value, &mut references)
                }
                Declaration::Embed(decl) => {
                    collect_embed_static_references(&decl.source, &mut references)
                }
                Declaration::Function(function) => {
                    collect_stmt_static_references(&function.body, &mut references)
                }
                _ => {}
            }
        }
        liveness.string_literals = references.string_literals;
        return liveness;
    }

    loop {
        let before = (
            liveness.constants.len(),
            liveness.globals.len(),
            liveness.embeds.len(),
            liveness.string_literals.len(),
        );
        for declaration in &program.declarations {
            match unwrapped_declaration(declaration) {
                Declaration::Const(decl) if liveness.constants.contains(&decl.name) => {
                    let mut references = StaticReferences::default();
                    collect_expr_static_references(&decl.value, &mut references);
                    apply_static_references(
                        &mut liveness,
                        &references,
                        &all_constants,
                        &all_globals,
                        &all_embeds,
                    );
                }
                Declaration::Global(decl) if liveness.globals.contains(&decl.name) => {
                    let mut references = StaticReferences::default();
                    collect_expr_static_references(&decl.value, &mut references);
                    apply_static_references(
                        &mut liveness,
                        &references,
                        &all_constants,
                        &all_globals,
                        &all_embeds,
                    );
                }
                Declaration::Embed(decl) if liveness.embeds.contains(&decl.name) => {
                    let mut references = StaticReferences::default();
                    collect_embed_static_references(&decl.source, &mut references);
                    if let Some(align) = &decl.align {
                        collect_expr_static_references(align, &mut references);
                    }
                    apply_static_references(
                        &mut liveness,
                        &references,
                        &all_constants,
                        &all_globals,
                        &all_embeds,
                    );
                }
                _ => {}
            }
        }
        let after = (
            liveness.constants.len(),
            liveness.globals.len(),
            liveness.embeds.len(),
            liveness.string_literals.len(),
        );
        if before == after {
            break;
        }
    }
    liveness
}

fn apply_static_references(
    liveness: &mut StaticLiveness,
    references: &StaticReferences,
    all_constants: &HashSet<String>,
    all_globals: &HashSet<String>,
    all_embeds: &HashSet<String>,
) {
    liveness
        .string_literals
        .extend(references.string_literals.iter().cloned());
    for name in &references.names {
        if all_constants.contains(name) {
            liveness.constants.insert(name.clone());
        }
        if all_globals.contains(name) {
            liveness.globals.insert(name.clone());
        }
        if all_embeds.contains(name) {
            liveness.embeds.insert(name.clone());
        }
    }
}

fn collect_stmt_static_references(stmts: &[Stmt], references: &mut StaticReferences) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::LetTwo { value, .. } | Stmt::Expr(value) => {
                collect_expr_static_references(value, references)
            }
            Stmt::Assign { target, value, .. } => {
                collect_place_static_references(target, references);
                collect_expr_static_references(value, references);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_static_references(condition, references);
                collect_stmt_static_references(then_body, references);
                collect_stmt_static_references(else_body, references);
            }
            Stmt::While { condition, body } => {
                collect_expr_static_references(condition, references);
                collect_stmt_static_references(body, references);
            }
            Stmt::Loop { body } => collect_stmt_static_references(body, references),
            Stmt::Return(Some(value)) | Stmt::Out { value, .. } => {
                collect_expr_static_references(value, references)
            }
            Stmt::ReturnTwo { first, second } => {
                collect_expr_static_references(first, references);
                collect_expr_static_references(second, references);
            }
            Stmt::Return(None) | Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => {}
        }
    }
}

fn collect_place_static_references(place: &Place, references: &mut StaticReferences) {
    match place {
        Place::Ident(name) => {
            references.names.insert(name.clone());
        }
        Place::Index { name, index } => {
            references.names.insert(name.clone());
            collect_expr_static_references(index, references);
        }
        Place::Field { base, .. } => {
            references.names.insert(base.clone());
        }
        Place::Access(path) => {
            references.names.insert(path.root.clone());
            collect_access_static_references(path, references);
        }
        Place::Deref(pointer) => collect_expr_static_references(pointer, references),
    }
}

fn collect_expr_static_references(expr: &Expr, references: &mut StaticReferences) {
    match expr {
        Expr::String(value) => {
            references.string_literals.insert(value.clone());
        }
        Expr::Ident(name)
        | Expr::Index { name, .. }
        | Expr::AddressOfIndex { name, .. }
        | Expr::AddressOf(name) => {
            references.names.insert(name.clone());
            match expr {
                Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
                    collect_expr_static_references(index, references)
                }
                _ => {}
            }
        }
        Expr::Field { base, field } | Expr::AddressOfField { base, field } => {
            references.names.insert(base.clone());
            references.names.insert(format!("{base}.{field}"));
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            references.names.insert(path.root.clone());
            collect_access_static_references(path, references);
        }
        Expr::Array(values) => {
            for value in values {
                collect_expr_static_references(value, references);
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_static_references(value, references);
            }
        }
        Expr::Deref(pointer)
        | Expr::Unary { expr: pointer, .. }
        | Expr::Cast { expr: pointer, .. }
        | Expr::BankedPointer { pointer, .. } => {
            collect_expr_static_references(pointer, references)
        }
        Expr::Call { path, args } => {
            references.names.insert(path_text(path));
            for arg in args {
                collect_expr_static_references(arg, references);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_static_references(left, references);
            collect_expr_static_references(right, references);
        }
        Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) | Expr::In(_) => {}
    }
}

fn collect_access_static_references(path: &AccessPath, references: &mut StaticReferences) {
    let mut qualified_name = path.root.clone();
    references.names.insert(qualified_name.clone());
    for segment in &path.segments {
        match segment {
            AccessSegment::Field(field) => {
                qualified_name.push('.');
                qualified_name.push_str(field);
                references.names.insert(qualified_name.clone());
            }
            AccessSegment::Index(index) => collect_expr_static_references(index, references),
        }
    }
}

fn collect_embed_static_references(
    source: &crate::ast::EmbedSource,
    references: &mut StaticReferences,
) {
    match source {
        crate::ast::EmbedSource::Bytes(values) => {
            for value in values {
                collect_expr_static_references(value, references);
            }
        }
        crate::ast::EmbedSource::Repeat { value, len } => {
            collect_expr_static_references(value, references);
            collect_expr_static_references(len, references);
        }
        crate::ast::EmbedSource::File(_)
        | crate::ast::EmbedSource::Text(_)
        | crate::ast::EmbedSource::CStr(_) => {}
    }
}

fn collect_stmt_function_references(stmts: &[Stmt], references: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. }
            | Stmt::LetTwo { value, .. }
            | Stmt::Return(Some(value))
            | Stmt::Expr(value)
            | Stmt::Out { value, .. } => collect_expr_function_references(value, references),
            Stmt::ReturnTwo { first, second } => {
                collect_expr_function_references(first, references);
                collect_expr_function_references(second, references);
            }
            Stmt::Assign { value, .. } => collect_expr_function_references(value, references),
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_function_references(condition, references);
                collect_stmt_function_references(then_body, references);
                collect_stmt_function_references(else_body, references);
            }
            Stmt::While { condition, body } => {
                collect_expr_function_references(condition, references);
                collect_stmt_function_references(body, references);
            }
            Stmt::Loop { body } => collect_stmt_function_references(body, references),
            Stmt::Return(None) | Stmt::Break | Stmt::Continue | Stmt::Asm { .. } => {}
        }
    }
}

fn collect_expr_function_references(expr: &Expr, references: &mut Vec<String>) {
    match expr {
        Expr::AddressOf(name) => references.push(name.clone()),
        Expr::Array(values) => values
            .iter()
            .for_each(|value| collect_expr_function_references(value, references)),
        Expr::Index { index, .. } | Expr::AddressOfIndex { index, .. } => {
            collect_expr_function_references(index, references)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_expr_function_references(index, references);
                }
            }
        }
        Expr::StructInit { fields, .. } => fields
            .iter()
            .for_each(|(_, value)| collect_expr_function_references(value, references)),
        Expr::Deref(value)
        | Expr::BankedPointer { pointer: value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Cast { expr: value, .. } => collect_expr_function_references(value, references),
        Expr::Call { args, .. } => args
            .iter()
            .for_each(|arg| collect_expr_function_references(arg, references)),
        Expr::Binary { left, right, .. } => {
            collect_expr_function_references(left, references);
            collect_expr_function_references(right, references);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. } => {}
    }
}

fn collect_stmt_calls(stmts: &[Stmt], calls: &mut Vec<String>) {
    collect_stmt_calls_with_symbols(stmts, calls, None)
}

fn collect_reachable_stmt_calls(stmts: &[Stmt], calls: &mut Vec<String>, symbols: &Symbols) {
    collect_stmt_calls_with_symbols(stmts, calls, Some(symbols))
}

fn collect_stmt_calls_with_symbols(
    stmts: &[Stmt],
    calls: &mut Vec<String>,
    symbols: Option<&Symbols>,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::LetTwo { value, .. } => {
                collect_expr_calls(value, calls)
            }
            Stmt::Assign { target, value, .. } => {
                collect_place_calls(target, calls);
                collect_expr_calls(value, calls);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_calls(condition, calls);
                if let Some(symbols) = symbols
                    && let Ok(value) = symbols.eval_i64(condition)
                {
                    if value == 0 {
                        collect_stmt_calls_with_symbols(else_body, calls, Some(symbols));
                    } else {
                        collect_stmt_calls_with_symbols(then_body, calls, Some(symbols));
                    }
                    if stmt_terminates_current_block(stmt) {
                        break;
                    }
                    continue;
                }
                collect_stmt_calls_with_symbols(then_body, calls, symbols);
                collect_stmt_calls_with_symbols(else_body, calls, symbols);
            }
            Stmt::While { condition, body } => {
                collect_expr_calls(condition, calls);
                if let Some(symbols) = symbols
                    && symbols.eval_i64(condition).is_ok_and(|value| value == 0)
                {
                    if stmt_terminates_current_block(stmt) {
                        break;
                    }
                    continue;
                }
                collect_stmt_calls_with_symbols(body, calls, symbols);
            }
            Stmt::Loop { body } => collect_stmt_calls_with_symbols(body, calls, symbols),
            Stmt::Return(Some(expr)) | Stmt::Expr(expr) => collect_expr_calls(expr, calls),
            Stmt::ReturnTwo { first, second } => {
                collect_expr_calls(first, calls);
                collect_expr_calls(second, calls);
            }
            Stmt::Out { value, .. } => collect_expr_calls(value, calls),
            Stmt::Asm { lines, .. } => {
                if let Some(symbols) = symbols {
                    for name in symbols.functions.keys() {
                        let label = function_label(name);
                        if lines
                            .iter()
                            .any(|line| inline_asm_references_label(line, &label))
                        {
                            calls.push(name.clone());
                        }
                    }
                }
            }
            Stmt::Break | Stmt::Continue | Stmt::Return(None) => {}
        }
        if stmt_terminates_current_block(stmt) {
            break;
        }
    }
}

fn collect_place_calls(place: &Place, calls: &mut Vec<String>) {
    match place {
        Place::Index { index, .. } | Place::Deref(index) => collect_expr_calls(index, calls),
        Place::Access(path) => collect_access_calls(path, calls),
        Place::Ident(_) | Place::Field { .. } => {}
    }
}

fn collect_expr_calls(expr: &Expr, calls: &mut Vec<String>) {
    match expr {
        Expr::Array(values) => {
            for value in values {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Index { index, .. }
        | Expr::AddressOfIndex { index, .. }
        | Expr::Deref(index)
        | Expr::BankedPointer { pointer: index, .. } => {
            collect_expr_calls(index, calls);
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => collect_access_calls(path, calls),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_calls(value, calls);
            }
        }
        Expr::Call { path, args } => {
            calls.push(path_text(path));
            for arg in args {
                collect_expr_calls(arg, calls);
            }
        }
        Expr::Unary { expr, .. } | Expr::Cast { expr, .. } => collect_expr_calls(expr, calls),
        Expr::Binary { left, right, .. } => {
            collect_expr_calls(left, calls);
            collect_expr_calls(right, calls);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. }
        | Expr::AddressOfField { .. }
        | Expr::AddressOf(_) => {}
    }
}

fn addr24(addr: i64) -> Option<u32> {
    if (0..=0xFF_FFFF).contains(&addr) {
        Some(addr as u32)
    } else {
        None
    }
}

fn collect_access_calls(path: &AccessPath, calls: &mut Vec<String>) {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            collect_expr_calls(index, calls);
        }
    }
}

fn const_shl_or_zero(left: i64, right: i64) -> i64 {
    if !(0..64).contains(&right) {
        0
    } else {
        left.wrapping_shl(right as u32)
    }
}

fn const_shr_or_zero(left: i64, right: i64, signed: bool) -> i64 {
    if right < 0 {
        return 0;
    }
    if signed {
        if right >= 64 {
            if left < 0 { -1 } else { 0 }
        } else {
            left >> right as u32
        }
    } else if right >= 64 {
        0
    } else {
        left.wrapping_shr(right as u32)
    }
}

fn path_text(path: &[String]) -> String {
    path.join(".")
}

fn module_alias_original_name(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(_, original)| original)
}

fn function_label(name: &str) -> String {
    let mut label = String::from("_");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            label.push(ch);
        } else {
            label.push('_');
        }
    }
    label
}

fn function_pointer_label(name: &str) -> String {
    format!(
        "__ezra_fn_ptr_{}",
        function_label(name).trim_start_matches('_')
    )
}

fn function_pointer_constant_label(target: &str) -> String {
    format!(
        "__ezra_fn_addr_{}",
        target.trim_start_matches('_').replace('.', "_")
    )
}

fn inline_asm_references_label(line: &str, label: &str) -> bool {
    line.split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.')))
        .any(|token| token.eq_ignore_ascii_case(label))
}

fn reserved_function_label(label: &str) -> bool {
    matches!(
        label,
        "__ezra_start"
            | "__ezra_exit"
            | "__ezra_pass"
            | "__ezra_fail"
            | "__ezra_memcpy"
            | "__ezra_memset"
            | "__ezra_mul_u8"
            | "__ezra_mul_u16"
            | "__ezra_mul_u24"
            | "__ezra_mul_i24"
            | "__ezra_div_u8"
            | "__ezra_div_u16"
            | "__ezra_div_u24"
            | "__ezra_div_i24"
            | "__ezra_mod_u8"
            | "__ezra_mod_u16"
            | "__ezra_mod_u24"
            | "__ezra_mod_i24"
    )
}

fn scalar_var(addr: u32, size: u32) -> Variable {
    Variable {
        addr,
        size,
        element_size: None,
        len: None,
    }
}

fn storage_ranges_overlap(left: Variable, right: Variable) -> bool {
    let left_start = u64::from(left.addr);
    let left_end = left_start + u64::from(left.size);
    let right_start = u64::from(right.addr);
    let right_end = right_start + u64::from(right.size);
    left_start < right_end && right_start < left_end
}

fn declaration_is_banked(declaration: &Declaration) -> bool {
    match declaration {
        Declaration::Bank { .. } => true,
        Declaration::Cfg { declaration, .. } => declaration_is_banked(declaration),
        _ => false,
    }
}

fn declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            declaration_name(declaration)
        }
        Declaration::Import(_) => None,
        Declaration::Const(decl) => Some(&decl.name),
        Declaration::Alias(decl) => Some(&decl.name),
        Declaration::Port(decl) => Some(&decl.name),
        Declaration::Mmio(decl) => Some(&decl.name),
        Declaration::Embed(decl) => Some(&decl.name),
        Declaration::Global(decl) => Some(&decl.name),
        Declaration::Struct(decl) => Some(&decl.name),
        Declaration::ExternAsmFunction(decl) => Some(&decl.name),
        Declaration::Function(decl) => Some(&decl.name),
    }
}

fn function_declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::ExternAsmFunction(decl) => Some(&decl.name),
        Declaration::Function(decl) => Some(&decl.name),
        _ => None,
    }
}

fn find_const_declaration<'a>(
    program: &'a Program,
    name: &str,
) -> Option<&'a crate::ast::ConstDecl> {
    program
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            Declaration::Const(decl) if decl.name == name => Some(decl),
            _ => None,
        })
}

fn collect_const_dependency_names(expr: &Expr, names: &mut Vec<String>) {
    match expr {
        Expr::Ident(name) => names.push(name.clone()),
        Expr::Field { base, field } => names.push(format!("{base}.{field}")),
        Expr::Access(path) => {
            if let Ok(name) = const_access_name(path) {
                names.push(name);
            }
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_dependency_names(index, names);
                }
            }
        }
        Expr::Unary { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::Deref(expr)
        | Expr::BankedPointer { pointer: expr, .. } => collect_const_dependency_names(expr, names),
        Expr::Binary { left, right, .. } => {
            collect_const_dependency_names(left, names);
            collect_const_dependency_names(right, names);
        }
        Expr::Array(values) => {
            for value in values {
                collect_const_dependency_names(value, names);
            }
        }
        Expr::Index { index, .. } => collect_const_dependency_names(index, names),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_const_dependency_names(value, names);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_const_dependency_names(arg, names);
            }
        }
        Expr::AddressOfIndex { index, .. } => collect_const_dependency_names(index, names),
        Expr::AddressOfAccess(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_dependency_names(index, names);
                }
            }
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::In(_)
        | Expr::AddressOf(_)
        | Expr::AddressOfField { .. } => {}
    }
}

fn collect_const_address_roots(expr: &Expr, roots: &mut Vec<String>) {
    match expr {
        Expr::AddressOf(name) => roots.push(name.clone()),
        Expr::AddressOfIndex { name, index } => {
            roots.push(name.clone());
            collect_const_address_roots(index, roots);
        }
        Expr::AddressOfField { base, .. } => roots.push(base.clone()),
        Expr::AddressOfAccess(path) => {
            roots.push(path.root.clone());
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_address_roots(index, roots);
                }
            }
        }
        Expr::Unary { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::Deref(expr)
        | Expr::BankedPointer { pointer: expr, .. } => collect_const_address_roots(expr, roots),
        Expr::Binary { left, right, .. } => {
            collect_const_address_roots(left, roots);
            collect_const_address_roots(right, roots);
        }
        Expr::Array(values) => {
            for value in values {
                collect_const_address_roots(value, roots);
            }
        }
        Expr::Index { index, .. } => {
            collect_const_address_roots(index, roots);
        }
        Expr::Access(path) => {
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_const_address_roots(index, roots);
                }
            }
        }
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_const_address_roots(value, roots);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_const_address_roots(arg, roots);
            }
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. } => {}
    }
}

fn has_attr(function: &Function, attr: &str) -> bool {
    function.attrs.iter().any(|candidate| candidate == attr)
}

fn validate_function_attrs(function: &Function) -> Result<(), Diagnostic> {
    let mut seen = HashSet::new();
    for attr in &function.attrs {
        if !seen.insert(attr.as_str()) {
            return Err(Diagnostic::new(format!(
                "duplicate attribute `{attr}` on function `{}`",
                function.name
            )));
        }
    }
    Ok(())
}

fn block_guarantees_value_return(stmts: &[Stmt], symbols: &Symbols) -> bool {
    stmts
        .iter()
        .any(|stmt| stmt_guarantees_value_return(stmt, symbols))
}

fn stmt_guarantees_value_return(stmt: &Stmt, symbols: &Symbols) -> bool {
    match stmt {
        Stmt::Return(Some(_)) | Stmt::ReturnTwo { .. } => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } if !else_body.is_empty() => {
            block_guarantees_value_return(then_body, symbols)
                && block_guarantees_value_return(else_body, symbols)
        }
        Stmt::Loop { body } => !block_can_break_current_loop(body),
        Stmt::While { condition, body } if condition_is_const_true(condition, symbols) => {
            !block_can_break_current_loop(body)
        }
        _ => false,
    }
}

fn block_guarantees_two_value_return(stmts: &[Stmt], symbols: &Symbols) -> bool {
    stmts
        .iter()
        .any(|stmt| stmt_guarantees_two_value_return(stmt, symbols))
}

fn stmt_guarantees_two_value_return(stmt: &Stmt, symbols: &Symbols) -> bool {
    match stmt {
        Stmt::ReturnTwo { .. } => true,
        Stmt::Return(Some(Expr::Call { path, .. }))
            if call_is_two_result(&path_text(path), symbols) =>
        {
            true
        }
        Stmt::If {
            then_body,
            else_body,
            ..
        } if !else_body.is_empty() => {
            block_guarantees_two_value_return(then_body, symbols)
                && block_guarantees_two_value_return(else_body, symbols)
        }
        Stmt::Loop { body } => !block_can_break_current_loop(body),
        Stmt::While { condition, body } if condition_is_const_true(condition, symbols) => {
            !block_can_break_current_loop(body)
        }
        _ => false,
    }
}

fn call_is_two_result(name: &str, symbols: &Symbols) -> bool {
    CATALOG
        .lookup(name)
        .is_some_and(|descriptor| descriptor.result_count == ResultCount::Two)
        || symbols
            .functions
            .get(name)
            .is_some_and(|sig| sig.second_return_type.is_some())
}

fn condition_is_const_true(condition: &Expr, symbols: &Symbols) -> bool {
    matches!(condition, Expr::Bool(true))
        || symbols.eval_i64(condition).is_ok_and(|value| value != 0)
}

fn block_terminates_current_block(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_terminates_current_block)
}

fn stmt_terminates_current_block(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::ReturnTwo { .. } | Stmt::Break | Stmt::Continue => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } if !else_body.is_empty() => {
            block_terminates_current_block(then_body) && block_terminates_current_block(else_body)
        }
        Stmt::Loop { body } => {
            !block_can_break_current_loop(body) && block_terminates_current_block(body)
        }
        Stmt::While {
            condition: Expr::Bool(true),
            body,
        } => !block_can_break_current_loop(body) && block_terminates_current_block(body),
        _ => false,
    }
}

fn collect_inline_asm_storage_requirements(
    stmts: &[Stmt],
    output_names: &mut HashSet<String>,
    clobbers_memory: &mut bool,
) {
    for stmt in stmts {
        match stmt {
            Stmt::Asm {
                outputs, clobbers, ..
            } => {
                output_names.extend(outputs.iter().map(|output| output.name.clone()));
                *clobbers_memory |= asm_clobbers_include(clobbers, "memory");
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_inline_asm_storage_requirements(then_body, output_names, clobbers_memory);
                collect_inline_asm_storage_requirements(else_body, output_names, clobbers_memory);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => {
                collect_inline_asm_storage_requirements(body, output_names, clobbers_memory);
            }
            _ => {}
        }
    }
}

fn assigned_names_in_block(stmts: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_assigned_names(stmts, &mut names);
    names
}

fn storage_required_names_in_block(stmts: &[Stmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    collect_storage_required_names(stmts, &mut names);
    names
}

fn collect_storage_required_names(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { value, .. } | Stmt::LetTwo { value, .. } | Stmt::Expr(value) => {
                collect_expr_storage_required_names(value, names)
            }
            Stmt::Assign { target, value, .. } => {
                collect_place_storage_required_names(target, names);
                collect_expr_storage_required_names(value, names);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_storage_required_names(condition, names);
                collect_storage_required_names(then_body, names);
                collect_storage_required_names(else_body, names);
            }
            Stmt::While { condition, body } => {
                collect_expr_storage_required_names(condition, names);
                collect_storage_required_names(body, names);
            }
            Stmt::Loop { body } => collect_storage_required_names(body, names),
            Stmt::Out { value, .. } => collect_expr_storage_required_names(value, names),
            Stmt::Return(Some(value)) => collect_expr_storage_required_names(value, names),
            Stmt::ReturnTwo { first, second } => {
                collect_expr_storage_required_names(first, names);
                collect_expr_storage_required_names(second, names);
            }
            Stmt::Asm {
                inputs, outputs, ..
            } => {
                names.extend(
                    inputs
                        .iter()
                        .filter(|input| input.class == "mem")
                        .map(|input| input.name.clone()),
                );
                names.extend(outputs.iter().map(|output| output.name.clone()));
            }
            Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        }
    }
}

fn collect_place_storage_required_names(place: &Place, names: &mut HashSet<String>) {
    match place {
        Place::Ident(_) | Place::Field { .. } => {}
        Place::Index { name, index } => {
            names.insert(name.clone());
            collect_expr_storage_required_names(index, names);
        }
        Place::Access(path) => collect_access_storage_required_names(path, names),
        Place::Deref(pointer) => {
            if let Expr::Ident(name) = pointer.as_ref() {
                names.insert(name.clone());
            }
            collect_expr_storage_required_names(pointer, names);
        }
    }
}

fn collect_expr_storage_required_names(expr: &Expr, names: &mut HashSet<String>) {
    match expr {
        Expr::AddressOf(name) => {
            names.insert(name.clone());
        }
        Expr::AddressOfIndex { name, index } => {
            names.insert(name.clone());
            collect_expr_storage_required_names(index, names);
        }
        Expr::AddressOfField { base, .. } => {
            names.insert(base.clone());
        }
        Expr::AddressOfAccess(path) => {
            names.insert(path.root.clone());
            collect_access_storage_required_names(path, names);
        }
        Expr::Array(values) => {
            for value in values {
                collect_expr_storage_required_names(value, names);
            }
        }
        Expr::Deref(pointer) => {
            if let Expr::Ident(name) = pointer.as_ref() {
                names.insert(name.clone());
            }
            collect_expr_storage_required_names(pointer, names);
        }
        Expr::Index { index, .. }
        | Expr::BankedPointer { pointer: index, .. }
        | Expr::Unary { expr: index, .. }
        | Expr::Cast { expr: index, .. } => collect_expr_storage_required_names(index, names),
        Expr::Access(path) => collect_access_storage_required_names(path, names),
        Expr::StructInit { fields, .. } => {
            for (_, value) in fields {
                collect_expr_storage_required_names(value, names);
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_storage_required_names(arg, names);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_storage_required_names(left, names);
            collect_expr_storage_required_names(right, names);
        }
        Expr::Int(_)
        | Expr::TypedInt(_, _)
        | Expr::Bool(_)
        | Expr::Char(_)
        | Expr::String(_)
        | Expr::Ident(_)
        | Expr::In(_)
        | Expr::Field { .. } => {}
    }
}

fn collect_access_storage_required_names(path: &AccessPath, names: &mut HashSet<String>) {
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            collect_expr_storage_required_names(index, names);
        }
    }
}

fn assigned_names_in_program(program: &Program) -> HashSet<String> {
    let mut names = HashSet::new();
    for declaration in &program.declarations {
        if let Declaration::Function(function) = declaration {
            collect_assigned_names(&function.body, &mut names);
        }
    }
    names
}

fn collect_assigned_names(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Assign { target, .. } => collect_assigned_place(target, names),
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_assigned_names(then_body, names);
                collect_assigned_names(else_body, names);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => collect_assigned_names(body, names),
            _ => {}
        }
    }
}

fn collect_assigned_place(place: &Place, names: &mut HashSet<String>) {
    if let Place::Ident(name) = place {
        names.insert(name.clone());
    }
}

fn block_can_break_current_loop(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_can_break_current_loop)
}

fn stmt_can_break_current_loop(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Break => true,
        Stmt::If {
            then_body,
            else_body,
            ..
        } => block_can_break_current_loop(then_body) || block_can_break_current_loop(else_body),
        Stmt::While { .. } | Stmt::Loop { .. } => false,
        _ => false,
    }
}

fn const_access_name(path: &AccessPath) -> Result<String, Diagnostic> {
    if path
        .segments
        .iter()
        .all(|segment| matches!(segment, AccessSegment::Field(_)))
    {
        Ok(access_path_summary(path))
    } else {
        Err(Diagnostic::new(
            "expression is not supported in a constant declaration",
        ))
    }
}

fn type_is_signed(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if matches!(name.as_str(), "i8" | "i16" | "i24"))
}

fn struct_scalar_type<'a>(
    ty: &'a Type,
    structs: &HashMap<String, StructLayout>,
) -> Option<&'a str> {
    match ty {
        Type::Named(name) if structs.contains_key(name) => Some(name.as_str()),
        _ => None,
    }
}

fn signed_min_bytes_for_size(size: u32) -> &'static [u8] {
    match size {
        1 => &[0x80],
        2 => &[0x00, 0x80],
        3 => &[0x00, 0x00, 0x80],
        _ => &[],
    }
}

fn signed_negative_one_bytes_for_size(size: u32) -> &'static [u8] {
    match size {
        1 => &[0xFF],
        2 => &[0xFF, 0xFF],
        3 => &[0xFF, 0xFF, 0xFF],
        _ => &[],
    }
}

fn signed_min_bytes(width: ValueWidth) -> &'static [u8] {
    match width {
        ValueWidth::U8 => &[0x80],
        ValueWidth::U16 => &[0x00, 0x80],
        ValueWidth::U24 => &[0x00, 0x00, 0x80],
    }
}

fn signed_negative_one_bytes(width: ValueWidth) -> &'static [u8] {
    match width {
        ValueWidth::U8 => &[0xFF],
        ValueWidth::U16 => &[0xFF, 0xFF],
        ValueWidth::U24 => &[0xFF, 0xFF, 0xFF],
    }
}

fn type_is_bool(ty: &Type) -> bool {
    matches!(ty, Type::Named(name) if name == "bool")
}

fn validate_integer_unary_operand_type(ty: &Type) -> Result<(), Diagnostic> {
    if type_is_bool(ty) || matches!(ty, Type::Ptr(_)) {
        return Err(Diagnostic::new("unary operand must be an integer"));
    }
    match ty {
        Type::Named(name)
            if matches!(name.as_str(), "u8" | "i8" | "u16" | "i16" | "u24" | "i24") =>
        {
            Ok(())
        }
        Type::Named(name) if name == "ptr" => {
            Err(Diagnostic::new("unary operand must be an integer"))
        }
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
            Err(Diagnostic::new(format!(
                "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
            )))
        }
        Type::Named(name) => Err(Diagnostic::new(format!("unknown type `{name}`"))),
        Type::Array { .. } => Err(Diagnostic::new("unary operand must be an integer")),
        Type::Ptr(_) | Type::Function { .. } => {
            Err(Diagnostic::new("unary operand must be an integer"))
        }
    }
}

fn validate_shift_operand_type(ty: &Type) -> Result<(), Diagnostic> {
    if type_is_bool(ty) || matches!(ty, Type::Ptr(_)) {
        return Err(Diagnostic::new("shift operand must be an integer"));
    }
    match ty {
        Type::Named(name)
            if matches!(name.as_str(), "u8" | "i8" | "u16" | "i16" | "u24" | "i24") =>
        {
            Ok(())
        }
        Type::Named(name) if name == "ptr" => {
            Err(Diagnostic::new("shift operand must be an integer"))
        }
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
            Err(Diagnostic::new(format!(
                "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
            )))
        }
        Type::Named(name) => Err(Diagnostic::new(format!("unknown type `{name}`"))),
        Type::Array { .. } => Err(Diagnostic::new("shift operand must be an integer")),
        Type::Ptr(_) | Type::Function { .. } => {
            Err(Diagnostic::new("shift operand must be an integer"))
        }
    }
}

fn validate_shift_count_integer_type(ty: &Type) -> Result<(), Diagnostic> {
    if type_is_bool(ty) || matches!(ty, Type::Ptr(_)) {
        return Err(Diagnostic::new("shift count must be an integer"));
    }
    match ty {
        Type::Named(name)
            if matches!(name.as_str(), "u8" | "i8" | "u16" | "i16" | "u24" | "i24") =>
        {
            Ok(())
        }
        Type::Named(name) if name == "ptr" => {
            Err(Diagnostic::new("shift count must be an integer"))
        }
        Type::Named(name) if matches!(name.as_str(), "u32" | "i32" | "u64" | "i64") => {
            Err(Diagnostic::new(format!(
                "type `{name}` is not supported; use explicit u8/u16/u24 or i8/i16/i24"
            )))
        }
        Type::Named(name) => Err(Diagnostic::new(format!("unknown type `{name}`"))),
        Type::Array { .. } => Err(Diagnostic::new("shift count must be an integer")),
        Type::Ptr(_) | Type::Function { .. } => {
            Err(Diagnostic::new("shift count must be an integer"))
        }
    }
}

fn validate_runtime_shift_count_type(ty: &Type) -> Result<(), Diagnostic> {
    match ty {
        Type::Named(name) if name == "u8" => Ok(()),
        _ => Err(Diagnostic::new("runtime shift count must be u8")),
    }
}

fn ptr_u8_type() -> Type {
    Type::Ptr(Box::new(Type::Named("u8".to_owned())))
}

fn width_unsigned_type(width: ValueWidth) -> Type {
    let name = match width {
        ValueWidth::U8 => "u8",
        ValueWidth::U16 => "u16",
        ValueWidth::U24 => "u24",
    };
    Type::Named(name.to_owned())
}

fn is_raw_address_type(name: &str) -> bool {
    matches!(name, "u24" | "ptr")
}

fn validate_comparison_types<F>(
    left_type: &Type,
    op: BinaryOp,
    right_type: &Type,
    widths: F,
) -> Result<(), Diagnostic>
where
    F: FnOnce() -> Option<(ValueWidth, ValueWidth)>,
{
    if matches!(left_type, Type::Array { .. }) || matches!(right_type, Type::Array { .. }) {
        return Err(Diagnostic::new("array value cannot be used as a scalar"));
    }
    if type_is_bool(left_type) || type_is_bool(right_type) {
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && left_type == right_type {
            return Ok(());
        }
        return Err(Diagnostic::new("type mismatch"));
    }

    let left_is_ptr = matches!(left_type, Type::Ptr(_) | Type::Function { .. });
    let right_is_ptr = matches!(right_type, Type::Ptr(_) | Type::Function { .. });
    if left_is_ptr || right_is_ptr {
        // Pointers of the same type share an address representation. The
        // backend lowers their ordering comparisons as unsigned wide compares.
        if left_type == right_type {
            return Ok(());
        }
        return Err(Diagnostic::new("type mismatch"));
    }

    if type_is_signed(left_type) != type_is_signed(right_type) {
        return Err(Diagnostic::new("signed/unsigned mix without cast"));
    }
    if let Some((left_width, right_width)) = widths()
        && left_width != right_width
    {
        return Err(Diagnostic::new(
            "comparison operands must have same width without cast",
        ));
    }
    Ok(())
}

fn int_value_type(value: i64) -> Type {
    if (0..=0xFF).contains(&value) {
        Type::Named("u8".to_owned())
    } else if (0..=0xFFFF).contains(&value) {
        Type::Named("u16".to_owned())
    } else {
        Type::Named("u24".to_owned())
    }
}

fn expr_is_untyped_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Int(_) | Expr::Char(_) => true,
        Expr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => matches!(expr.as_ref(), Expr::Int(_)),
        _ => false,
    }
}

fn format_immediate(value: i64, width: ValueWidth) -> String {
    match width {
        ValueWidth::U8 => format!("{:02X}h", (value as u64) & 0xFF),
        ValueWidth::U16 => format!("{:04X}h", (value as u64) & 0xFFFF),
        ValueWidth::U24 => format!("{:06X}h", (value as u64) & 0xFF_FFFF),
    }
}

fn validate_main_signature(main: &Function) -> Result<(), Diagnostic> {
    if !main.params.is_empty() {
        return Err(Diagnostic::new("main function cannot take parameters"));
    }
    if main.return_type.is_some() {
        return Err(Diagnostic::new("main function cannot return a value"));
    }
    Ok(())
}

fn validate_inline_asm_clobbers(
    clobbers: &[String],
    lines: &[String],
    allow_sp_clobber: bool,
    cpu: AssemblerCpu,
) -> Result<(), Diagnostic> {
    let mut seen = HashSet::new();
    for clobber in clobbers {
        if !is_allowed_inline_asm_clobber(clobber) {
            return Err(Diagnostic::new(format!(
                "unknown inline asm clobber `{clobber}`"
            )));
        }
        if !seen.insert(clobber.as_str()) {
            return Err(Diagnostic::new(format!(
                "duplicate inline asm clobber `{clobber}`"
            )));
        }
    }
    if asm_clobbers_include(clobbers, "sp") && !allow_sp_clobber {
        return Err(Diagnostic::new(
            "inline asm clobber `sp` is only allowed in naked functions",
        ));
    }
    for line in lines {
        if is_unsupported_z80_family_instruction(cpu, line)? {
            return Err(Diagnostic::new(format!(
                "test assembler does not support instruction `{}`",
                line.trim()
            )));
        }
        let effects = analyze_instruction(cpu, line)?.effects;
        for register in effects.referenced_special_registers {
            if !asm_clobbers_include(clobbers, register) {
                return Err(Diagnostic::new(format!(
                    "inline asm uses `{register}` without declaring clobber `{register}`"
                )));
            }
        }
        if effects.uses_ports && !asm_clobbers_include(clobbers, "ports") {
            return Err(Diagnostic::new(
                "inline asm uses ports without declaring clobber `ports`",
            ));
        }
        if effects.changes_flags && !asm_clobbers_include_flags(clobbers) {
            return Err(Diagnostic::new(
                "inline asm changes flags without declaring clobber `flags`",
            ));
        }
        if effects.uses_memory && !asm_clobbers_include(clobbers, "memory") {
            return Err(Diagnostic::new(
                "inline asm uses memory without declaring clobber `memory`",
            ));
        }
        for register in effects.modified_registers {
            if !asm_clobbers_include_register(clobbers, register) {
                return Err(Diagnostic::new(format!(
                    "inline asm modifies `{register}` without declaring clobber `{register}`"
                )));
            }
        }
    }
    Ok(())
}

fn is_allowed_inline_asm_clobber(clobber: &str) -> bool {
    matches!(
        clobber,
        "a" | "f"
            | "af"
            | "b"
            | "c"
            | "bc"
            | "d"
            | "e"
            | "de"
            | "h"
            | "l"
            | "hl"
            | "ix"
            | "iy"
            | "sp"
            | "memory"
            | "ports"
            | "flags"
    )
}

fn asm_clobbers_include(clobbers: &[String], name: &str) -> bool {
    clobbers.iter().any(|clobber| clobber == name)
}

fn asm_clobbers_include_flags(clobbers: &[String]) -> bool {
    asm_clobbers_include(clobbers, "flags")
        || asm_clobbers_include(clobbers, "f")
        || asm_clobbers_include(clobbers, "af")
}

fn asm_clobbers_include_register(clobbers: &[String], register: &str) -> bool {
    if asm_clobbers_include(clobbers, register) {
        return true;
    }
    match register {
        "a" | "f" => asm_clobbers_include(clobbers, "af"),
        "b" | "c" => asm_clobbers_include(clobbers, "bc"),
        "d" | "e" => asm_clobbers_include(clobbers, "de"),
        "h" | "l" => asm_clobbers_include(clobbers, "hl"),
        "af" => {
            asm_clobbers_include(clobbers, "a")
                && (asm_clobbers_include(clobbers, "f") || asm_clobbers_include(clobbers, "flags"))
        }
        "bc" => asm_clobbers_include(clobbers, "b") && asm_clobbers_include(clobbers, "c"),
        "de" => asm_clobbers_include(clobbers, "d") && asm_clobbers_include(clobbers, "e"),
        "hl" => asm_clobbers_include(clobbers, "h") && asm_clobbers_include(clobbers, "l"),
        _ => false,
    }
}

fn substitute_inline_asm_operands(
    line: &str,
    operands: &HashMap<String, String>,
) -> Result<String, Diagnostic> {
    let mut output = String::new();
    let mut rest = line;
    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('}') else {
            return Err(Diagnostic::new(format!(
                "unterminated inline asm operand placeholder in `{line}`"
            )));
        };
        let name = &after_start[..end];
        let Some(binding) = operands.get(name) else {
            return Err(Diagnostic::new(format!(
                "unknown inline asm operand placeholder `{name}`"
            )));
        };
        output.push_str(binding);
        rest = &after_start[end + 1..];
    }
    if rest.contains('}') {
        return Err(Diagnostic::new(format!(
            "unmatched inline asm operand placeholder in `{line}`"
        )));
    }
    output.push_str(rest);
    Ok(output)
}

fn is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
    )
}

fn sdk_constants(options: &AssemblyOptions) -> HashMap<String, i64> {
    let mut constants = HashMap::from([
        ("EZRA_LOAD_ADDR".to_owned(), options.load_addr.get() as i64),
        (
            "EZRA_ENTRY_ADDR".to_owned(),
            options.entry_addr.get() as i64,
        ),
        ("EZRA_CODE_BASE".to_owned(), options.code_base.get() as i64),
        ("EZRA_STACK_TOP".to_owned(), options.stack_top.get() as i64),
        ("EZRA_RAM_BASE".to_owned(), options.ram_base.get() as i64),
        ("EZRA_VRAM_BASE".to_owned(), options.vram_base.get() as i64),
        (
            "EZRA_AUDIO_BASE".to_owned(),
            options.audio_base.get() as i64,
        ),
        (
            "EZRA_ASSET_BASE".to_owned(),
            options.asset_base.get() as i64,
        ),
        (
            "EZRA_RODATA_BASE".to_owned(),
            options.rodata_base.get() as i64,
        ),
    ]);
    if options.default_sdk_symbols {
        constants.extend([
            ("VRAM_BASE".to_owned(), options.vram_base.get() as i64),
            ("AUDIO_BASE".to_owned(), options.audio_base.get() as i64),
            ("BTN_B".to_owned(), 0x0001),
            ("BTN_Y".to_owned(), 0x0002),
            ("BTN_SELECT".to_owned(), 0x0004),
            ("BTN_START".to_owned(), 0x0008),
            ("BTN_UP".to_owned(), 0x0010),
            ("BTN_DOWN".to_owned(), 0x0020),
            ("BTN_LEFT".to_owned(), 0x0040),
            ("BTN_RIGHT".to_owned(), 0x0080),
            ("BTN_A".to_owned(), 0x0100),
            ("BTN_X".to_owned(), 0x0200),
            ("BTN_L".to_owned(), 0x0400),
            ("BTN_R".to_owned(), 0x0800),
            ("VIDEO_PRESENT".to_owned(), 1),
            ("VIDEO_CLEAR".to_owned(), 2),
            ("VIDEO_SET_MODE".to_owned(), 3),
            ("AUDIO_SUBMIT_BUFFER".to_owned(), 1),
            ("AUDIO_STOP".to_owned(), 2),
        ]);
    }
    constants
}

fn sdk_constant_types(options: &AssemblyOptions) -> HashMap<String, Type> {
    let mut types = HashMap::new();
    for name in [
        "EZRA_LOAD_ADDR",
        "EZRA_ENTRY_ADDR",
        "EZRA_CODE_BASE",
        "EZRA_STACK_TOP",
        "EZRA_RAM_BASE",
        "EZRA_VRAM_BASE",
        "EZRA_AUDIO_BASE",
        "EZRA_ASSET_BASE",
        "EZRA_RODATA_BASE",
    ] {
        types.insert(name.to_owned(), Type::Named("u24".to_owned()));
    }
    if !options.default_sdk_symbols {
        return types;
    }
    for name in ["VRAM_BASE", "AUDIO_BASE"] {
        types.insert(
            name.to_owned(),
            Type::Ptr(Box::new(Type::Named("u8".to_owned()))),
        );
    }
    for name in [
        "BTN_B",
        "BTN_Y",
        "BTN_SELECT",
        "BTN_START",
        "BTN_UP",
        "BTN_DOWN",
        "BTN_LEFT",
        "BTN_RIGHT",
        "BTN_A",
        "BTN_X",
        "BTN_L",
        "BTN_R",
    ] {
        types.insert(name.to_owned(), Type::Named("u16".to_owned()));
    }
    for name in [
        "VIDEO_PRESENT",
        "VIDEO_CLEAR",
        "VIDEO_SET_MODE",
        "AUDIO_SUBMIT_BUFFER",
        "AUDIO_STOP",
    ] {
        types.insert(name.to_owned(), Type::Named("u8".to_owned()));
    }
    types
}

fn sdk_ports(options: &AssemblyOptions) -> HashMap<String, u8> {
    if !options.default_sdk_symbols {
        return HashMap::new();
    }
    HashMap::from([
        ("PAD1_LO".to_owned(), 0x01),
        ("PAD1_HI".to_owned(), 0x02),
        ("PAD2_LO".to_owned(), 0x03),
        ("PAD2_HI".to_owned(), 0x04),
        ("PAD3_LO".to_owned(), 0x05),
        ("PAD3_HI".to_owned(), 0x06),
        ("PAD4_LO".to_owned(), 0x07),
        ("PAD4_HI".to_owned(), 0x08),
        ("VIDEO_CMD".to_owned(), 0x09),
        ("AUDIO_CMD".to_owned(), 0x0A),
        ("SYS_STATUS".to_owned(), 0x0B),
        ("DEBUG_CHAR".to_owned(), 0x0C),
        ("TEST_RESULT".to_owned(), 0x0D),
        ("TEST_HALT".to_owned(), 0x0E),
        ("EXT_ADDR0".to_owned(), 0x10),
        ("EXT_ADDR1".to_owned(), 0x11),
        ("EXT_ADDR2".to_owned(), 0x12),
        ("EXT_LEN0".to_owned(), 0x13),
        ("EXT_LEN1".to_owned(), 0x14),
        ("EXT_MODE".to_owned(), 0x15),
        ("EXT_COMMAND".to_owned(), 0x16),
        ("EXT_STATUS".to_owned(), 0x17),
    ])
}

#[cfg(feature = "std")]
fn read_embed_file(path: &str, source_path: &SourcePath) -> Result<Vec<u8>, Diagnostic> {
    let path = Path::new(path);
    if path.is_absolute() {
        return read_embed_file_candidate(path);
    }

    let candidates = embed_file_candidates(path, source_path);
    let missing_path = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| path.to_path_buf());
    for candidate in candidates {
        match fs::read(&candidate) {
            Ok(bytes) => return Ok(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Diagnostic::new(format!(
                    "failed to read embedded file `{}`: {error}",
                    candidate.display()
                )));
            }
        }
    }
    Err(Diagnostic::new(format!(
        "embedded file `{}` not found",
        missing_path.display()
    )))
}

#[cfg(feature = "std")]
fn read_embed_file_candidate(path: &Path) -> Result<Vec<u8>, Diagnostic> {
    fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Diagnostic::new(format!("embedded file `{}` not found", path.display()))
        } else {
            Diagnostic::new(format!(
                "failed to read embedded file `{}`: {error}",
                path.display()
            ))
        }
    })
}

#[cfg(feature = "std")]
fn embed_file_candidates(path: &Path, source_path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![
        source_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path),
    ];
    if let Ok(project_root) = std::env::current_dir() {
        let project_relative = project_root.join(path);
        if !candidates
            .iter()
            .any(|candidate| candidate == &project_relative)
        {
            candidates.push(project_relative);
        }
    }
    candidates
}

#[cfg(all(feature = "no-std", not(feature = "std")))]
fn read_embed_file(path: &str, _source_path: &SourcePath) -> Result<Vec<u8>, Diagnostic> {
    Err(Diagnostic::new(format!(
        "embedded file `{path}` is unavailable without a host filesystem"
    )))
}

#[cfg(test)]
mod tests;
