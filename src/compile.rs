#[cfg(feature = "avr")]
use crate::asm::emit_avr_assembly_with_options;
#[cfg(feature = "dcpu")]
use crate::asm::emit_dcpu_assembly_with_options;
#[cfg(feature = "i8086")]
use crate::asm::emit_i8086_assembly_with_options;
#[cfg(feature = "m6800")]
use crate::asm::emit_m6800_assembly_with_options;
#[cfg(feature = "tms9900")]
use crate::asm::emit_tms9900_assembly_with_options;

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    asm::ez80::emitter::collect_ez80_semantic_diagnostics,
    asm::{
        AssemblyOptions, emit_ez80_assembly_with_options, emit_lr35902_assembly_with_options,
        emit_mos6502_assembly_with_options,
    },
    ast::{
        AccessPath, AccessSegment, CfgPredicate, Declaration, EmbedSource, Expr, Function, Place,
        Program, Stmt, Type,
    },
    diagnostic::{Diagnostic, SourceLocation, diagnostic_span},
    layout::{Layout, default_layout_for_target},
    parser::parse_program,
    target::{
        Address24, AssemblerCpu, CpuFamily, DEFAULT_TARGET_TRIPLE, memory_model_for_cpu,
        parse_target_triple,
    },
    workspace::{Workspace, materialize_workspace_embeds},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileOptions {
    pub source: PathBuf,
    pub debug_comments: bool,
    pub default_sdk_symbols: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileReport {
    pub imports: usize,
    pub declarations: usize,
    pub has_main: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SdkResolver {
    pub target: Option<String>,
    pub sdk_roots: Vec<PathBuf>,
}

pub fn check_source(source: &str, options: &CompileOptions) -> Result<CompileReport, Diagnostic> {
    check_source_with_sdk(source, options, &SdkResolver::default())
}

pub fn check_source_diagnostics(source: &str, options: &CompileOptions) -> Vec<Diagnostic> {
    check_source_diagnostics_with_sdk(source, options, &SdkResolver::default())
}

pub fn check_source_diagnostics_with_sdk(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
) -> Vec<Diagnostic> {
    check_source_diagnostics_with_sdk_and_overrides(source, options, sdk, &HashMap::new())
}

pub fn check_source_diagnostics_with_sdk_and_overrides(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
) -> Vec<Diagnostic> {
    check_diagnostics_with_sdk_and_overrides(source, options, sdk, source_overrides, true, true)
}

pub fn check_source_semantic_diagnostics_with_sdk_and_overrides(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
) -> Vec<Diagnostic> {
    check_diagnostics_with_sdk_and_overrides(source, options, sdk, source_overrides, true, false)
}

pub fn check_module_diagnostics_with_sdk_and_overrides(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
) -> Vec<Diagnostic> {
    check_diagnostics_with_sdk_and_overrides(source, options, sdk, source_overrides, false, false)
}

fn check_diagnostics_with_sdk_and_overrides(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
    require_main: bool,
    validate_codegen: bool,
) -> Vec<Diagnostic> {
    let root = match parse_program(&options.source, source) {
        Ok(program) => program,
        Err(error) => return vec![error],
    };
    let mut source_program = root.clone();
    source_program.declarations = match active_declarations(source_program.declarations, sdk) {
        Ok(declarations) => declarations,
        Err(error) => return vec![locate_source_diagnostic(error, source, &options.source)],
    };
    let resolved = match resolve_program_imports(
        root,
        sdk,
        &mut Vec::new(),
        &mut HashSet::new(),
        source_overrides,
        None,
    ) {
        Ok(program) => program,
        Err(error) => return vec![locate_source_diagnostic(error, source, &options.source)],
    };
    let mut diagnostics =
        collect_reference_diagnostics(&source_program, &resolved, options.default_sdk_symbols);
    for unit in &resolved.source_units {
        if normalize_path(&unit.path) == normalize_path(&source_program.source_path) {
            continue;
        }
        if let Ok(mut program) = parse_program(&unit.path, &unit.text)
            && let Ok(declarations) = active_declarations(program.declarations, sdk)
        {
            program.declarations = declarations;
            diagnostics.extend(collect_reference_diagnostics(
                &program,
                &resolved,
                options.default_sdk_symbols,
            ));
        }
    }
    let cpu = sdk
        .target
        .as_deref()
        .and_then(|target| parse_target_triple(target).ok())
        .map(|target| target.cpu)
        .unwrap_or(CpuFamily::Ez80);
    if matches!(
        cpu,
        CpuFamily::Ez80
            | CpuFamily::Z80
            | CpuFamily::Z80N
            | CpuFamily::Z180
            | CpuFamily::I8080
            | CpuFamily::I8085
            | CpuFamily::Lr35902
            | CpuFamily::Avr
            | CpuFamily::M6800
    ) {
        for diagnostic in collect_ez80_semantic_diagnostics(
            &resolved,
            diagnostic_assembly_options(
                sdk.target.as_deref(),
                cpu,
                options.debug_comments,
                options.default_sdk_symbols,
            ),
        ) {
            if !diagnostics
                .iter()
                .any(|existing| diagnostic_is_covered_by(existing, &diagnostic))
            {
                diagnostics.push(diagnostic);
            }
        }
    }
    let final_error = if require_main && validate_codegen {
        check_source_with_sdk_and_overrides(source, options, sdk, source_overrides).err()
    } else if require_main {
        match resolved.main_function() {
            Some(main) => validate_main_signature(main)
                .map_err(|error| locate_source_diagnostic(error, source, &options.source))
                .err(),
            None => Some(Diagnostic::at(
                source_start_location(&options.source),
                "missing required `fn main()`",
            )),
        }
    } else {
        None
    };
    if let Some(error) = final_error
        && !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == error.message)
    {
        diagnostics.push(error);
    }
    diagnostics
}

fn diagnostic_assembly_options(
    target: Option<&str>,
    cpu: CpuFamily,
    debug_comments: bool,
    default_sdk_symbols: bool,
) -> AssemblyOptions {
    let target = target.unwrap_or(DEFAULT_TARGET_TRIPLE);
    let layout = layout_for_target(target, cpu);
    let symbol = |name: &str| {
        layout
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .map(|symbol| symbol.value)
    };
    let is_16_bit = memory_model_for_cpu(cpu).is_some_and(|model| model.address_width_bits <= 16);
    let ram_fallback = is_16_bit.then_some(Address24::new(0xA000));
    let rodata_fallback = is_16_bit.then_some(Address24::new(0x8000));
    let asset_fallback = is_16_bit.then_some(Address24::new(0xC000));
    let low_fallback = is_16_bit.then_some(Address24::new(0));
    let defaults = AssemblyOptions::default();

    AssemblyOptions {
        cpu,
        debug_comments,
        default_sdk_symbols,
        dos_executable: target == crate::target::MSDOS_COM_I8086_TARGET,
        mos_executable: layout.name == "agon_light_mos",
        c64_executable: matches!(layout.name.as_str(), "commodore64_6502" | "commodore64_crt"),
        ti_os_executable: target.starts_with("ti83-z80")
            || target.starts_with("ti83plus-z80")
            || target.starts_with("ti84-z80")
            || target.starts_with("ti84plus-z80")
            || target.starts_with("ti84plusce-ez80")
            || target.starts_with("ti83premiumce-ez80"),
        arduboy_executable: target.starts_with("arduboy-"),
        load_addr: symbol("EZRA_LOAD_ADDR").unwrap_or(layout.load),
        entry_addr: symbol("EZRA_ENTRY_ADDR").unwrap_or(layout.entry),
        code_base: symbol("EZRA_CODE_BASE").unwrap_or(layout.entry),
        stack_top: symbol("EZRA_STACK_TOP").unwrap_or(layout.stack),
        ram_base: symbol("EZRA_RAM_BASE")
            .or(ram_fallback)
            .unwrap_or(defaults.ram_base),
        vram_base: symbol("EZRA_VRAM_BASE")
            .or(low_fallback)
            .unwrap_or(defaults.vram_base),
        audio_base: symbol("EZRA_AUDIO_BASE")
            .or(low_fallback)
            .unwrap_or(defaults.audio_base),
        asset_base: symbol("EZRA_ASSET_BASE")
            .or(asset_fallback)
            .unwrap_or(defaults.asset_base),
        rodata_base: symbol("EZRA_RODATA_BASE")
            .or(rodata_fallback)
            .unwrap_or(defaults.rodata_base),
        section_bases: Vec::new(),
    }
}

fn layout_for_target(target: &str, cpu: CpuFamily) -> Layout {
    let layout = default_layout_for_target(target);
    if cpu == CpuFamily::I8086 && layout_requires_more_than_16_bits(&layout) {
        Layout::bare_16(cpu.as_str())
    } else {
        layout
    }
}

fn layout_requires_more_than_16_bits(layout: &Layout) -> bool {
    layout.load.get() > 0xFFFF
        || layout.entry.get() > 0xFFFF
        || layout.stack.get() > 0xFFFF
        || layout
            .regions
            .iter()
            .any(|region| region.start.get() > 0xFFFF || region.end.get() > 0xFFFF)
        || layout
            .symbols
            .iter()
            .any(|symbol| symbol.value.get() > 0xFFFF)
}

fn validate_layout_for_cpu(
    layout: &Layout,
    cpu: CpuFamily,
    target: &str,
) -> Result<(), Diagnostic> {
    if let Err(errors) = layout.validate() {
        return Err(Diagnostic::new(format!(
            "layout `{}` is invalid: {}",
            layout.name,
            errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    let address_width_bits = memory_model_for_cpu(cpu)
        .map(|memory| memory.address_width_bits)
        .ok_or_else(|| Diagnostic::new(format!("CPU `{}` has no memory model", cpu.as_str())))?;
    let max_addr = if address_width_bits >= 24 {
        Address24::MAX
    } else {
        (1u32 << address_width_bits) - 1
    };
    let mut violations = Vec::new();
    if layout.load.get() > max_addr {
        violations.push(format!("load address {}", layout.load));
    }
    if layout.entry.get() > max_addr {
        violations.push(format!("entry address {}", layout.entry));
    }
    if layout.stack.get() > max_addr {
        violations.push(format!("stack address {}", layout.stack));
    }
    for region in &layout.regions {
        if region.start.get() > max_addr || region.end.get() > max_addr {
            violations.push(format!(
                "region `{}` range {}..{}",
                region.name, region.start, region.end
            ));
        }
    }
    for symbol in &layout.symbols {
        if symbol.value.get() > max_addr {
            violations.push(format!("symbol `{}` value {}", symbol.name, symbol.value));
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(Diagnostic::new(format!(
            "layout `{}` requires addresses outside the {}-bit address space for target `{target}`: {}",
            layout.name,
            address_width_bits,
            violations.join(", ")
        )))
    }
}

fn validate_text_section_fit(layout: &Layout, code_len: usize) -> Result<(), Diagnostic> {
    let section = layout
        .sections
        .iter()
        .find(|section| section.name == ".text")
        .ok_or_else(|| {
            Diagnostic::new(format!("layout `{}` has no section `.text`", layout.name))
        })?;
    let region = layout
        .regions
        .iter()
        .find(|region| region.name == section.region)
        .ok_or_else(|| {
            Diagnostic::new(format!(
                "layout section `.text` targets unknown region `{}`",
                section.region
            ))
        })?;
    let end = if code_len == 0 {
        layout.entry.get()
    } else {
        layout
            .entry
            .get()
            .checked_add(
                u32::try_from(code_len)
                    .map_err(|_| Diagnostic::new("program code exceeds 24-bit address space"))?
                    - 1,
            )
            .ok_or_else(|| Diagnostic::new("section `.text` exceeds 24-bit address space"))?
    };
    if layout.entry.get() < region.start.get() || end > region.end.get() {
        return Err(Diagnostic::new(format!(
            "section `.text` does not fit in region `{}`",
            region.name
        )));
    }
    Ok(())
}

fn diagnostic_is_covered_by(existing: &Diagnostic, candidate: &Diagnostic) -> bool {
    if existing.message != candidate.message {
        return false;
    }
    let (Some(existing), Some(candidate)) = (&existing.span, &candidate.span) else {
        return false;
    };
    existing.file == candidate.file
        && (candidate.start.line, candidate.start.column)
            <= (existing.start.line, existing.start.column)
        && (existing.end.line, existing.end.column) <= (candidate.end.line, candidate.end.column)
}

pub fn check_source_with_sdk(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
) -> Result<CompileReport, Diagnostic> {
    check_source_with_sdk_and_overrides(source, options, sdk, &HashMap::new())
}

pub fn check_source_with_sdk_and_overrides(
    source: &str,
    options: &CompileOptions,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
) -> Result<CompileReport, Diagnostic> {
    let root = parse_program(&options.source, source)?;
    let imports = root
        .declarations
        .iter()
        .filter(|decl| matches!(decl, crate::ast::Declaration::Import(_)))
        .count();
    let fallback_location = source_start_location(&options.source);
    let program = resolve_program_imports(
        root,
        sdk,
        &mut Vec::new(),
        &mut HashSet::new(),
        source_overrides,
        None,
    )
    .map_err(|error| locate_source_diagnostic(error, source, &options.source))?;
    let declarations = program.declarations.len();
    let has_main = program.main_function().is_some();

    if !has_main {
        return Err(Diagnostic::at(
            fallback_location,
            "missing required `fn main()`",
        ));
    }
    validate_main_signature(program.main_function().expect("main presence checked"))
        .map_err(|error| locate_source_diagnostic(error, source, &options.source))?;
    let cpu = sdk
        .target
        .as_deref()
        .map(parse_target_triple)
        .transpose()
        .map_err(Diagnostic::new)?
        .map(|target| target.cpu)
        .unwrap_or(CpuFamily::Ez80);
    let target_name = sdk.target.as_deref().unwrap_or(DEFAULT_TARGET_TRIPLE);
    let layout = layout_for_target(target_name, cpu);
    validate_layout_for_cpu(&layout, cpu, target_name)?;
    let assembly_options = diagnostic_assembly_options(
        sdk.target.as_deref(),
        cpu,
        options.debug_comments,
        options.default_sdk_symbols,
    );
    let assembly = if cpu == CpuFamily::I8086 {
        #[cfg(feature = "i8086")]
        {
            emit_i8086_assembly_with_options(&program, assembly_options)
        }
        #[cfg(not(feature = "i8086"))]
        {
            Err(Diagnostic::new(
                "i8086 source compilation requires the `i8086` Cargo feature",
            ))
        }
    } else if cpu == CpuFamily::Lr35902 {
        emit_lr35902_assembly_with_options(&program, assembly_options)
    } else if cpu == CpuFamily::Avr {
        #[cfg(feature = "avr")]
        {
            emit_avr_assembly_with_options(&program, assembly_options)
        }
        #[cfg(not(feature = "avr"))]
        {
            unreachable!("AVR targets require the avr Cargo feature")
        }
    } else if matches!(
        cpu,
        CpuFamily::Mos6502 | CpuFamily::Cmos65C02 | CpuFamily::Wdc65C816 | CpuFamily::Ricoh2A03
    ) {
        emit_mos6502_assembly_with_options(&program, assembly_options)
    } else if cpu == CpuFamily::Dcpu {
        #[cfg(feature = "dcpu")]
        {
            emit_dcpu_assembly_with_options(&program, assembly_options)
        }
        #[cfg(not(feature = "dcpu"))]
        {
            Err(Diagnostic::new(
                "DCPU-16 targets require the dcpu Cargo feature",
            ))
        }
    } else if cpu == CpuFamily::M6800 {
        #[cfg(feature = "m6800")]
        {
            emit_m6800_assembly_with_options(&program, assembly_options)
        }
        #[cfg(not(feature = "m6800"))]
        {
            unreachable!("M6800 targets require the m6800 Cargo feature")
        }
    } else if cpu == CpuFamily::M68k {
        #[cfg(feature = "m68k")]
        {
            crate::asm::emit_m68k_assembly_with_options(&program, assembly_options)
        }
        #[cfg(not(feature = "m68k"))]
        {
            unreachable!("m68k targets require the m68k Cargo feature")
        }
    } else if cpu == CpuFamily::Tms9900 {
        #[cfg(feature = "tms9900")]
        {
            emit_tms9900_assembly_with_options(&program, assembly_options)
        }
        #[cfg(not(feature = "tms9900"))]
        {
            unreachable!("TMS9900 targets require the tms9900 Cargo feature")
        }
    } else {
        emit_ez80_assembly_with_options(&program, assembly_options)
    }
    .map_err(|error| locate_source_diagnostic(error, source, &options.source))?;
    let assembled = crate::vm::assemble_subset_with_symbols_at(
        AssemblerCpu::from(cpu),
        &assembly,
        layout.entry.get(),
    )
    .map_err(|error| error.with_location_if_missing(source_start_location(&options.source)))?;
    validate_text_section_fit(&layout, assembled.bytes.len())
        .map_err(|error| error.with_location_if_missing(source_start_location(&options.source)))?;

    Ok(CompileReport {
        imports,
        declarations,
        has_main,
    })
}

fn locate_source_diagnostic(error: Diagnostic, source: &str, path: &Path) -> Diagnostic {
    if error.location().is_some() {
        return error;
    }
    diagnostic_span(path, source, &error.message)
        .map(|span| error.clone().with_span_if_missing(span))
        .unwrap_or_else(|| error.with_location_if_missing(source_start_location(path)))
}

struct ReferenceDiagnostics {
    references: Vec<crate::ast::SourceReference>,
    used_spans: Vec<crate::diagnostic::SourceSpan>,
    diagnostics: Vec<Diagnostic>,
}

impl ReferenceDiagnostics {
    fn set_references(&mut self, references: &[crate::ast::SourceReference]) {
        self.references = references.to_vec();
        self.used_spans.clear();
    }

    fn push(&mut self, name: &str, message: String) {
        let matching = self
            .references
            .iter()
            .filter(|reference| normalize_reference(&reference.text) == normalize_reference(name))
            .collect::<Vec<_>>();
        let span = matching
            .iter()
            .copied()
            .filter(|reference| !self.used_spans.contains(&reference.span))
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
            .map(|reference| reference.span.clone())
            .or_else(|| {
                matching
                    .iter()
                    .copied()
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
                    .map(|reference| reference.span.clone())
            });
        if let Some(span) = span {
            self.used_spans.push(span.clone());
            self.diagnostics.push(Diagnostic::at_span(span, message));
        } else {
            self.diagnostics.push(Diagnostic::new(message));
        }
    }
}

fn normalize_reference(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn collect_reference_diagnostics(
    source_program: &Program,
    resolved: &Program,
    default_sdk_symbols: bool,
) -> Vec<Diagnostic> {
    let globals = resolved
        .declarations
        .iter()
        .filter_map(declaration_name)
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let mut output = ReferenceDiagnostics {
        references: Vec::new(),
        used_spans: Vec::new(),
        diagnostics: Vec::new(),
    };
    for declaration in &source_program.declarations {
        collect_declaration_references(declaration, &globals, default_sdk_symbols, &mut output);
    }
    output.diagnostics
}

fn collect_declaration_references(
    declaration: &Declaration,
    globals: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics,
) {
    match declaration {
        Declaration::Const(decl) => {
            collect_expr_references(&decl.value, globals, default_sdk_symbols, output)
        }
        Declaration::Port(decl) => {
            collect_expr_references(&decl.value, globals, default_sdk_symbols, output)
        }
        Declaration::Mmio(decl) => {
            collect_expr_references(&decl.value, globals, default_sdk_symbols, output)
        }
        Declaration::Global(decl) => {
            collect_expr_references(&decl.value, globals, default_sdk_symbols, output)
        }
        Declaration::Function(function) => {
            let mut names = globals.clone();
            names.extend(function.params.iter().map(|param| param.name.clone()));
            collect_local_names(&function.body, &mut names);
            collect_stmt_references(
                &function.body,
                &function.body_spans,
                &names,
                default_sdk_symbols,
                output,
            );
        }
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            collect_declaration_references(declaration, globals, default_sdk_symbols, output)
        }
        Declaration::Embed(_)
        | Declaration::Import(_)
        | Declaration::Alias(_)
        | Declaration::Struct(_)
        | Declaration::ExternAsmFunction(_) => {}
    }
}

fn collect_local_names(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Let { name, .. } => {
                names.insert(name.clone());
            }
            Stmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_local_names(then_body, names);
                collect_local_names(else_body, names);
            }
            Stmt::While { body, .. } | Stmt::Loop { body } => collect_local_names(body, names),
            Stmt::Asm { outputs, .. } => {
                names.extend(outputs.iter().map(|output| output.name.clone()));
            }
            _ => {}
        }
    }
}

fn collect_stmt_references(
    stmts: &[Stmt],
    spans: &[crate::ast::StmtSpan],
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics,
) {
    for (index, stmt) in stmts.iter().enumerate() {
        let span = spans.get(index);
        output.set_references(span.map_or(&[], |span| span.references.as_slice()));
        let child_spans: &[crate::ast::StmtSpan] =
            span.map_or(&[], |span| span.children.as_slice());
        match stmt {
            Stmt::Let { value, .. } | Stmt::Return(Some(value)) => {
                collect_expr_references(value, names, default_sdk_symbols, output)
            }
            Stmt::Assign { target, value, .. } => {
                collect_place_references(target, names, default_sdk_symbols, output);
                collect_expr_references(value, names, default_sdk_symbols, output);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expr_references(condition, names, default_sdk_symbols, output);
                let then_count = then_body.len();
                collect_stmt_references(
                    then_body,
                    &child_spans[..child_spans.len().min(then_count)],
                    names,
                    default_sdk_symbols,
                    output,
                );
                collect_stmt_references(
                    else_body,
                    child_spans.get(then_count..).unwrap_or_default(),
                    names,
                    default_sdk_symbols,
                    output,
                );
            }
            Stmt::While { condition, body } => {
                collect_expr_references(condition, names, default_sdk_symbols, output);
                collect_stmt_references(body, child_spans, names, default_sdk_symbols, output);
            }
            Stmt::Loop { body } => {
                collect_stmt_references(body, child_spans, names, default_sdk_symbols, output)
            }
            Stmt::Out { port, value } => {
                push_unknown_reference(port, false, names, default_sdk_symbols, output);
                collect_expr_references(value, names, default_sdk_symbols, output);
            }
            Stmt::Expr(expr) => collect_expr_references(expr, names, default_sdk_symbols, output),
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Asm { .. } => {}
        }
    }
}

fn collect_place_references(
    place: &Place,
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics,
) {
    match place {
        Place::Ident(name) => {
            push_unknown_reference(name, false, names, default_sdk_symbols, output)
        }
        Place::Index { name, index } => {
            push_unknown_reference(name, false, names, default_sdk_symbols, output);
            collect_expr_references(index, names, default_sdk_symbols, output);
        }
        Place::Field { base, field } => {
            push_unknown_field(base, field, names, default_sdk_symbols, output)
        }
        Place::Access(path) => {
            push_unknown_access_path(path, names, default_sdk_symbols, output);
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_expr_references(index, names, default_sdk_symbols, output);
                }
            }
        }
        Place::Deref(expr) => collect_expr_references(expr, names, default_sdk_symbols, output),
    }
}

fn collect_expr_references(
    expr: &Expr,
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics,
) {
    match expr {
        Expr::Ident(name) | Expr::In(name) | Expr::AddressOf(name) => {
            push_unknown_reference(name, false, names, default_sdk_symbols, output)
        }
        Expr::Index { name, index } | Expr::AddressOfIndex { name, index } => {
            push_unknown_reference(name, false, names, default_sdk_symbols, output);
            collect_expr_references(index, names, default_sdk_symbols, output);
        }
        Expr::Field { base, field } | Expr::AddressOfField { base, field } => {
            push_unknown_field(base, field, names, default_sdk_symbols, output)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            push_unknown_access_path(path, names, default_sdk_symbols, output);
            for segment in &path.segments {
                if let AccessSegment::Index(index) = segment {
                    collect_expr_references(index, names, default_sdk_symbols, output);
                }
            }
        }
        Expr::StructInit { ty, fields } => {
            push_unknown_reference(ty, false, names, default_sdk_symbols, output);
            for (_, value) in fields {
                collect_expr_references(value, names, default_sdk_symbols, output);
            }
        }
        Expr::Call { path, args } => {
            push_unknown_reference(&path.join("."), true, names, default_sdk_symbols, output);
            for arg in args {
                collect_expr_references(arg, names, default_sdk_symbols, output);
            }
        }
        Expr::Unary { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::Deref(expr)
        | Expr::BankedPointer { pointer: expr, .. } => {
            collect_expr_references(expr, names, default_sdk_symbols, output)
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_references(left, names, default_sdk_symbols, output);
            collect_expr_references(right, names, default_sdk_symbols, output);
        }
        Expr::Array(values) => {
            for value in values {
                collect_expr_references(value, names, default_sdk_symbols, output);
            }
        }
        Expr::Int(_) | Expr::TypedInt(_, _) | Expr::Bool(_) | Expr::Char(_) | Expr::String(_) => {}
    }
}

fn push_unknown_access_path(
    path: &AccessPath,
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics,
) {
    if names.contains(&path.root) {
        return;
    }
    let mut qualified = path.root.clone();
    for segment in &path.segments {
        let AccessSegment::Field(field) = segment else {
            break;
        };
        qualified.push('.');
        qualified.push_str(field);
    }
    push_unknown_reference(&qualified, false, names, default_sdk_symbols, output);
}

fn push_unknown_field(
    base: &str,
    field: &str,
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics,
) {
    if names.contains(base) {
        return;
    }
    push_unknown_reference(
        &format!("{base}.{field}"),
        false,
        names,
        default_sdk_symbols,
        output,
    );
}

fn push_unknown_reference(
    name: &str,
    function: bool,
    names: &HashSet<String>,
    default_sdk_symbols: bool,
    output: &mut ReferenceDiagnostics,
) {
    if names.contains(name)
        || default_sdk_symbols
            && matches!(
                name.split('.').next(),
                Some("test" | "debug" | "mem" | "ezra")
            )
    {
        return;
    }
    let kind = if function { "function" } else { "value" };
    output.push(name, format!("unknown {kind} `{name}`"));
}

fn source_start_location(path: &Path) -> SourceLocation {
    SourceLocation {
        file: path.to_path_buf(),
        line: 1,
        column: 1,
    }
}

pub fn load_program(path: &Path) -> Result<Program, Diagnostic> {
    load_program_with_sdk(path, &SdkResolver::default())
}

pub fn load_program_with_sdk(path: &Path, sdk: &SdkResolver) -> Result<Program, Diagnostic> {
    let source = fs::read_to_string(path).map_err(|error| {
        Diagnostic::new(format!("failed to read `{}`: {error}", path.display()))
    })?;
    parse_and_resolve_imports_with_sdk(path, &source, sdk)
}

pub fn parse_and_resolve_imports(path: &Path, source: &str) -> Result<Program, Diagnostic> {
    parse_and_resolve_imports_with_sdk(path, source, &SdkResolver::default())
}

pub fn parse_and_resolve_imports_with_sdk(
    path: &Path,
    source: &str,
    sdk: &SdkResolver,
) -> Result<Program, Diagnostic> {
    parse_and_resolve_imports_with_sdk_and_overrides(path, source, sdk, &HashMap::new())
}

/// Resolve imports using caller-provided source files before falling back to the host filesystem.
///
/// This is the import-resolution primitive used by the virtual-workspace API.
pub fn parse_and_resolve_imports_with_sdk_and_overrides(
    path: &Path,
    source: &str,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
) -> Result<Program, Diagnostic> {
    let root = parse_program(path, source)?;
    let mut stack = Vec::new();
    let mut seen = HashSet::new();
    resolve_program_imports(root, sdk, &mut stack, &mut seen, source_overrides, None)
}

pub fn parse_and_resolve_imports_with_sdk_and_workspace(
    path: &Path,
    source: &str,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
    workspace: &Workspace<'_>,
) -> Result<Program, Diagnostic> {
    let root = parse_program(path, source)?;
    let mut stack = Vec::new();
    let mut seen = HashSet::new();
    resolve_program_imports(
        root,
        sdk,
        &mut stack,
        &mut seen,
        source_overrides,
        Some(workspace),
    )
}

pub fn resolve_import_source(
    importer: &Path,
    import: &str,
    sdk: &SdkResolver,
) -> Result<(PathBuf, String), Diagnostic> {
    read_import_source(importer, import, sdk)
}

fn resolve_program_imports(
    mut program: Program,
    sdk: &SdkResolver,
    stack: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    source_overrides: &HashMap<PathBuf, String>,
    workspace: Option<&Workspace<'_>>,
) -> Result<Program, Diagnostic> {
    if let Some(workspace) = workspace {
        materialize_workspace_embeds(&mut program, workspace)?;
    }
    let path = normalize_path(&program.source_path);
    if stack.contains(&path) {
        let mut cycle = stack
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        cycle.push(path.display().to_string());
        return Err(Diagnostic::new(format!(
            "cyclic import detected: {}",
            cycle.join(" -> ")
        )));
    }
    if !seen.insert(path.clone()) {
        return Ok(Program {
            source_path: program.source_path,
            source_text: program.source_text,
            source_units: Vec::new(),
            declarations: Vec::new(),
        });
    }

    program.declarations = active_declarations(program.declarations, sdk)?;

    validate_private_import_access(&program, sdk, source_overrides)?;

    let short_module_counts = direct_import_short_module_counts(&program);
    stack.push(path);
    let mut declarations = Vec::new();
    let mut source_units = Vec::new();
    for declaration in &program.declarations {
        let Declaration::Import(import) = declaration else {
            continue;
        };
        let (import_path, source) =
            read_import_source_with_overrides(&program.source_path, import, sdk, source_overrides)?;
        // A module reached through an earlier import already contributed its
        // declarations and aliases. Skipping it here prevents shared SDK
        // dependencies from creating duplicate qualified aliases.
        let normalized_import_path = normalize_path(&import_path);
        if seen.contains(&normalized_import_path) && !stack.contains(&normalized_import_path) {
            continue;
        }
        let source = source_override(source_overrides, &import_path).unwrap_or(source);
        let mut imported = parse_program(&import_path, &source)?;
        if let Some(workspace) = workspace {
            materialize_workspace_embeds(&mut imported, workspace)?;
        }
        imported.declarations = active_declarations(imported.declarations, sdk)?;
        let short_module = import.rsplit('.').next().unwrap_or(import);
        let include_short_aliases = short_module_counts
            .get(short_module)
            .copied()
            .unwrap_or_default()
            <= 1;
        let module_aliases =
            module_alias_declarations(import, &imported.declarations, include_short_aliases);
        let imported =
            resolve_program_imports(imported, sdk, stack, seen, source_overrides, workspace)?;
        source_units.extend(imported.source_units.iter().cloned());
        let imported_declarations = imported
            .declarations
            .into_iter()
            .filter(|declaration| !is_entry_function(declaration))
            .collect::<Vec<_>>();
        reject_import_declaration_collisions(
            &declarations,
            &imported_declarations,
            &import_path,
            &source,
        )?;
        declarations.extend(imported_declarations);
        declarations.extend(module_aliases);
    }
    stack.pop();

    declarations.extend(
        program
            .declarations
            .into_iter()
            .filter(|decl| !matches!(decl, Declaration::Import(_))),
    );
    source_units.extend(program.source_units);

    Ok(Program {
        source_path: program.source_path,
        source_text: program.source_text,
        source_units,
        declarations,
    })
}

fn source_override(source_overrides: &HashMap<PathBuf, String>, path: &Path) -> Option<String> {
    source_overrides
        .get(path)
        .or_else(|| source_overrides.get(&normalize_path(path)))
        .cloned()
        .or_else(|| {
            let portable = path.to_string_lossy().replace('\\', "/");
            source_overrides
                .iter()
                .find(|(candidate, _)| candidate.to_string_lossy().replace('\\', "/") == portable)
                .map(|(_, source)| source.clone())
        })
}

fn reject_import_declaration_collisions(
    existing: &[Declaration],
    incoming: &[Declaration],
    import_path: &Path,
    source: &str,
) -> Result<(), Diagnostic> {
    let existing = existing
        .iter()
        .filter_map(declaration_name)
        .collect::<HashSet<_>>();
    for declaration in incoming {
        let Some(name) = declaration_name(declaration) else {
            continue;
        };
        if existing.contains(name) {
            let message = format!("duplicate imported declaration `{name}`");
            let diagnostic = diagnostic_span(import_path, source, &message)
                .map(|span| Diagnostic::at_span(span, message.clone()))
                .unwrap_or_else(|| Diagnostic::new(message));
            return Err(diagnostic);
        }
    }
    Ok(())
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

fn is_entry_function(declaration: &Declaration) -> bool {
    match declaration {
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            is_entry_function(declaration)
        }
        Declaration::Function(function) => function.name == "main",
        _ => false,
    }
}

fn active_declarations(
    declarations: Vec<Declaration>,
    sdk: &SdkResolver,
) -> Result<Vec<Declaration>, Diagnostic> {
    let context = CfgContext::new(sdk)?;
    declarations
        .into_iter()
        .filter_map(|declaration| active_declaration(declaration, &context).transpose())
        .collect()
}

fn active_declaration(
    declaration: Declaration,
    context: &CfgContext,
) -> Result<Option<Declaration>, Diagnostic> {
    match declaration {
        Declaration::Cfg {
            predicates,
            declaration,
        } => {
            for predicate in &predicates {
                if !context.matches(predicate)? {
                    return Ok(None);
                }
            }
            active_declaration(*declaration, context)
        }
        Declaration::Bank { bank, declaration } => {
            active_declaration(*declaration, context).map(|declaration| {
                declaration.map(|declaration| Declaration::Bank {
                    bank,
                    declaration: Box::new(declaration),
                })
            })
        }
        declaration => Ok(Some(declaration)),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CfgContext {
    target: String,
    cpu: String,
    target_family: Option<String>,
    vendor: Option<String>,
    os: Option<String>,
    features: HashSet<String>,
    pointer_width: u16,
    address_width: u16,
    debug: bool,
}

impl CfgContext {
    fn new(sdk: &SdkResolver) -> Result<Self, Diagnostic> {
        let target = sdk.target.as_deref().unwrap_or(DEFAULT_TARGET_TRIPLE);
        let triple = parse_target_triple(target).map_err(Diagnostic::new)?;
        let parts = target.split('-').collect::<Vec<_>>();
        let cpu = triple.cpu.as_str().to_owned();
        let target_family = parts.first().map(|part| (*part).to_owned());
        let vendor = parts.get(1).map(|part| (*part).to_owned());
        let os = parts
            .iter()
            .copied()
            .find(|part| matches!(*part, "mos" | "cpm" | "baremetal"))
            .map(str::to_owned);
        let features = parts
            .iter()
            .copied()
            .filter(|part| *part != cpu)
            .map(str::to_owned)
            .collect();
        let memory = memory_model_for_cpu(triple.cpu).ok_or_else(|| {
            Diagnostic::new(format!("no target profile is implemented for CPU `{cpu}`"))
        })?;
        Ok(Self {
            target: target.to_owned(),
            cpu,
            target_family,
            vendor,
            os,
            features,
            pointer_width: memory.pointer_width_bits,
            address_width: memory.address_width_bits,
            debug: cfg!(debug_assertions),
        })
    }

    fn matches(&self, predicate: &CfgPredicate) -> Result<bool, Diagnostic> {
        match predicate {
            CfgPredicate::Target(value) => Ok(self.target == *value),
            CfgPredicate::TargetFamily(value) => Ok(self.target_family.as_deref() == Some(value)),
            CfgPredicate::Cpu(value) => Ok(self.cpu == *value),
            CfgPredicate::Vendor(value) => Ok(self.vendor.as_deref() == Some(value)),
            CfgPredicate::Os(value) => Ok(self.os.as_deref() == Some(value)),
            CfgPredicate::PointerWidth(value) => Ok(self.pointer_width == *value),
            CfgPredicate::AddressWidth(value) => Ok(self.address_width == *value),
            CfgPredicate::Feature(value) => {
                if !self.features.contains(value) {
                    return Err(Diagnostic::new(format!("unknown cfg feature `{value}`")));
                }
                Ok(true)
            }
            CfgPredicate::Debug => Ok(self.debug),
            CfgPredicate::Release => Ok(!self.debug),
            CfgPredicate::All(predicates) => {
                for predicate in predicates {
                    if !self.matches(predicate)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            CfgPredicate::Any(predicates) => {
                for predicate in predicates {
                    if self.matches(predicate)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CfgPredicate::Not(predicate) => Ok(!self.matches(predicate)?),
        }
    }
}

fn read_import_source(
    source_path: &Path,
    import: &str,
    sdk: &SdkResolver,
) -> Result<(PathBuf, String), Diagnostic> {
    read_import_source_with_overrides(source_path, import, sdk, &HashMap::new())
}

fn read_import_source_with_overrides(
    source_path: &Path,
    import: &str,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
) -> Result<(PathBuf, String), Diagnostic> {
    let candidates = import_file_candidates(source_path, import, sdk);
    let missing_path = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(import.replace('.', "/")).with_extension("ezra"));
    for candidate in candidates {
        if let Some(source) = source_override(source_overrides, &candidate) {
            return Ok((candidate, source));
        }
        match fs::read_to_string(&candidate) {
            Ok(source) => return Ok((candidate, source)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Diagnostic::new(format!(
                    "failed to read import `{import}` at `{}`: {error}",
                    candidate.display()
                )));
            }
        }
    }
    if let Some(source) = builtin_sdk_source(sdk.target.as_deref(), import) {
        return Ok((
            builtin_sdk_path(sdk.target.as_deref(), import),
            source.to_owned(),
        ));
    }
    Err(Diagnostic::new(format!(
        "failed to read import `{import}` at `{}`: not found",
        missing_path.display()
    )))
}

fn import_file_candidates(source_path: &Path, import: &str, sdk: &SdkResolver) -> Vec<PathBuf> {
    let module_path = PathBuf::from(import.replace('.', "/")).with_extension("ezra");
    let source_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    let mut candidates = Vec::new();
    push_unique_path(&mut candidates, source_dir.join(&module_path));

    for ancestor in source_dir.ancestors().skip(1) {
        push_unique_path(&mut candidates, ancestor.join(&module_path));
    }

    if let Ok(project_root) = std::env::current_dir() {
        push_unique_path(&mut candidates, project_root.join(&module_path));
    }
    for root in &sdk.sdk_roots {
        push_unique_path(&mut candidates, root.join(&module_path));
    }
    candidates
}

fn builtin_sdk_path(target: Option<&str>, import: &str) -> PathBuf {
    if target == Some(crate::target::MSDOS_COM_I8086_TARGET) && import.starts_with("dos.") {
        PathBuf::from(format!(
            "toolchains/msdos-i8086/sdk/{}.ezra",
            import.replace('.', "/")
        ))
    } else {
        PathBuf::from(format!("builtin-sdk/{}.ezra", import.replace('.', "/")))
    }
}

fn builtin_sdk_source(target: Option<&str>, import: &str) -> Option<&'static str> {
    if target == Some(crate::target::MSDOS_COM_I8086_TARGET) {
        match import {
            "dos.constants" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/msdos-i8086/sdk/dos/constants.ezra"),
                "dos.constants",
            )),
            "dos.raw" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/msdos-i8086/sdk/dos/raw.ezra"),
                "dos.raw",
            )),
            "dos.console" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/msdos-i8086/sdk/dos/console.ezra"),
                "dos.console",
            )),
            "dos.file" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/msdos-i8086/sdk/dos/file.ezra"),
                "dos.file",
            )),
            "dos.directory" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/msdos-i8086/sdk/dos/directory.ezra"),
                "dos.directory",
            )),
            "dos.memory" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/msdos-i8086/sdk/dos/memory.ezra"),
                "dos.memory",
            )),
            "dos.datetime" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/msdos-i8086/sdk/dos/datetime.ezra"),
                "dos.datetime",
            )),
            "dos.process" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/msdos-i8086/sdk/dos/process.ezra"),
                "dos.process",
            )),
            "dos.psp" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/msdos-i8086/sdk/dos/psp.ezra"),
                "dos.psp",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("arduboy-")) {
        match import {
            "arduboy.core" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/arduboy-avr/sdk/arduboy/core.ezra"),
                "arduboy.core",
            )),
            "arduboy.input" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/arduboy-avr/sdk/arduboy/input.ezra"),
                "arduboy.input",
            )),
            "arduboy.oled" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/arduboy-avr/sdk/arduboy/oled.ezra"),
                "arduboy.oled",
            )),
            "arduboy.eeprom" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/arduboy-avr/sdk/arduboy/eeprom.ezra"),
                "arduboy.eeprom",
            )),
            "arduboy.timing" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/arduboy-avr/sdk/arduboy/timing.ezra"),
                "arduboy.timing",
            )),
            "arduboy.audio" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/arduboy-avr/sdk/arduboy/audio.ezra"),
                "arduboy.audio",
            )),
            "arduboy.graphics" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/arduboy-avr/sdk/arduboy/graphics.ezra"),
                "arduboy.graphics",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("gameboy-")) {
        match import {
            "gb.video" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/gameboy-lr35902/sdk/gb/video.ezra"),
                "gb.video",
            )),
            "gb.sprites" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/gameboy-lr35902/sdk/gb/sprites.ezra"),
                "gb.sprites",
            )),
            "gb.serial" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/gameboy-lr35902/sdk/gb/serial.ezra"),
                "gb.serial",
            )),
            "gb.input" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/gameboy-lr35902/sdk/gb/input.ezra"),
                "gb.input",
            )),
            "gb.audio" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/gameboy-lr35902/sdk/gb/audio.ezra"),
                "gb.audio",
            )),
            "gb.color" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/gameboy-lr35902/sdk/gb/color.ezra"),
                "gb.color",
            )),
            "gb.text" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/gameboy-lr35902/sdk/gb/text.ezra"),
                "gb.text",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("agonlight-mos-ez80")) {
        match import {
            "agon.buffers" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/buffers.ezra"),
                "agon.buffers",
            )),
            "agon.console" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/console.ezra"),
                "agon.console",
            )),
            "agon.mos" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/mos.ezra"),
                "agon.mos",
            )),
            "agon.gpio" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/gpio.ezra"),
                "agon.gpio",
            )),
            "agon.keyboard" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/keyboard.ezra"),
                "agon.keyboard",
            )),
            "agon.mouse" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/mouse.ezra"),
                "agon.mouse",
            )),
            "agon.sprites" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/sprites.ezra"),
                "agon.sprites",
            )),
            "agon.vdp" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/vdp.ezra"),
                "agon.vdp",
            )),
            "agon.text" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/agonlight-mos-ez80/sdk/agon/text.ezra"),
                "agon.text",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("ez180n-ez80")) {
        match import {
            "ez180n.console" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ez180n-ez80/sdk/ez180n/console.ezra"),
                "ez180n.console",
            )),
            _ => None,
        }
    } else if target.is_some_and(is_ti_ce_target) {
        match import {
            "tice.os" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/tice-ez80/sdk/tice/os.ezra"),
                "tice.os",
            )),
            "tice.lcd" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/tice-ez80/sdk/tice/lcd.ezra"),
                "tice.lcd",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("ti99-4a-tms9900")) {
        match import {
            "ti99.console" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti99-4a-tms9900/sdk/ti99/console.ezra"),
                "ti99.console",
            )),
            "ti99.graphics" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti99-4a-tms9900/sdk/ti99/graphics.ezra"),
                "ti99.graphics",
            )),
            "ti99.input" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti99-4a-tms9900/sdk/ti99/input.ezra"),
                "ti99.input",
            )),
            "ti99.sprites" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti99-4a-tms9900/sdk/ti99/sprites.ezra"),
                "ti99.sprites",
            )),
            "ti99.memory" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti99-4a-tms9900/sdk/ti99/memory.ezra"),
                "ti99.memory",
            )),
            "ti99.sound" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti99-4a-tms9900/sdk/ti99/sound.ezra"),
                "ti99.sound",
            )),
            "ti99.vdp" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti99-4a-tms9900/sdk/ti99/vdp.ezra"),
                "ti99.vdp",
            )),
            _ => None,
        }
    } else if target.is_some_and(is_ti_z80_target) {
        match import {
            "ti.os" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti-z80/sdk/ti/os.ezra"),
                "ti.os",
            )),
            "ti.lcd" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ti-z80/sdk/ti/lcd.ezra"),
                "ti.lcd",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("commodore64-6502")) {
        match import {
            "c64.vic" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/commodore64-6502/sdk/c64/vic.ezra"),
                "c64.vic",
            )),
            "c64.sid" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/commodore64-6502/sdk/c64/sid.ezra"),
                "c64.sid",
            )),
            "c64.cia" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/commodore64-6502/sdk/c64/cia.ezra"),
                "c64.cia",
            )),
            "c64.memory" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/commodore64-6502/sdk/c64/memory.ezra"),
                "c64.memory",
            )),
            "c64.kernal" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/commodore64-6502/sdk/c64/kernal.ezra"),
                "c64.kernal",
            )),
            "c64.text" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/commodore64-6502/sdk/c64/text.ezra"),
                "c64.text",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("zxspectrum-z80")) {
        match import {
            "zx.rom" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/zxspectrum-z80/sdk/zx/rom.ezra"),
                "zx.rom",
            )),
            "zx.screen" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/zxspectrum-z80/sdk/zx/screen.ezra"),
                "zx.screen",
            )),
            "zx.io" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/zxspectrum-z80/sdk/zx/io.ezra"),
                "zx.io",
            )),
            "zx.keyboard" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/zxspectrum-z80/sdk/zx/keyboard.ezra"),
                "zx.keyboard",
            )),
            "zx.sound" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/zxspectrum-z80/sdk/zx/sound.ezra"),
                "zx.sound",
            )),
            "zx.memory" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/zxspectrum-z80/sdk/zx/memory.ezra"),
                "zx.memory",
            )),
            "zx.interrupt" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/zxspectrum-z80/sdk/zx/interrupt.ezra"),
                "zx.interrupt",
            )),
            "zx.text" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/zxspectrum-z80/sdk/zx/text.ezra"),
                "zx.text",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.starts_with("ezra-test-")) {
        match import {
            "harness.io" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ezra-test-ez80/sdk/harness/io.ezra"),
                "harness.io",
            )),
            "harness.layout" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ezra-test-ez80/sdk/harness/layout.ezra"),
                "harness.layout",
            )),
            "harness.memory" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/ezra-test-ez80/sdk/harness/memory.ezra"),
                "harness.memory",
            )),
            _ => None,
        }
    } else if target.is_some_and(|target| target.split('-').any(|part| part == "cpm")) {
        match import {
            "cpm.bdos" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/cpm-2.2-z80/sdk/cpm/bdos.ezra"),
                "cpm.bdos",
            )),
            "cpm.console" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/cpm-2.2-z80/sdk/cpm/console.ezra"),
                "cpm.console",
            )),
            "cpm.text" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/cpm-2.2-z80/sdk/cpm/text.ezra"),
                "cpm.text",
            )),
            "cpm.dma" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/cpm-2.2-z80/sdk/cpm/dma.ezra"),
                "cpm.dma",
            )),
            "cpm.fcb" => Some(builtin_sdk_utf8(
                include_bytes!("../toolchains/cpm-2.2-z80/sdk/cpm/fcb.ezra"),
                "cpm.fcb",
            )),
            _ => None,
        }
    } else {
        None
    }
}

/// Return the built-in SDK modules available for the selected target.
///
/// Keep this list derived from `builtin_sdk_source` so consumers such as the
/// LSP cannot advertise a module that import resolution would reject.
pub fn builtin_sdk_modules(target: Option<&str>) -> Vec<&'static str> {
    const MODULES: &[&str] = &[
        "dos.constants",
        "dos.raw",
        "dos.console",
        "dos.file",
        "dos.directory",
        "dos.memory",
        "dos.datetime",
        "dos.process",
        "dos.psp",
        "arduboy.core",
        "arduboy.input",
        "arduboy.oled",
        "arduboy.eeprom",
        "arduboy.timing",
        "arduboy.audio",
        "arduboy.graphics",
        "gb.video",
        "gb.sprites",
        "gb.serial",
        "gb.input",
        "gb.audio",
        "gb.color",
        "gb.text",
        "agon.buffers",
        "agon.console",
        "agon.mos",
        "agon.gpio",
        "agon.keyboard",
        "agon.mouse",
        "agon.sprites",
        "agon.vdp",
        "agon.text",
        "ez180n.console",
        "tice.os",
        "tice.lcd",
        "ti.os",
        "ti.lcd",
        "ti99.console",
        "ti99.graphics",
        "ti99.input",
        "ti99.sprites",
        "ti99.memory",
        "ti99.sound",
        "ti99.vdp",
        "zx.rom",
        "zx.screen",
        "zx.io",
        "zx.keyboard",
        "zx.sound",
        "zx.memory",
        "zx.interrupt",
        "zx.text",
        "c64.vic",
        "c64.sid",
        "c64.cia",
        "c64.memory",
        "c64.kernal",
        "c64.text",
        "harness.io",
        "harness.layout",
        "harness.memory",
        "cpm.bdos",
        "cpm.console",
        "cpm.text",
        "cpm.dma",
        "cpm.fcb",
    ];

    MODULES
        .iter()
        .copied()
        .filter(|module| builtin_sdk_source(target, module).is_some())
        .collect()
}

fn is_ti_ce_target(target: &str) -> bool {
    target.starts_with("ti84plusce-ez80") || target.starts_with("ti83premiumce-ez80")
}

fn is_ti_z80_target(target: &str) -> bool {
    target.starts_with("ti83-z80")
        || target.starts_with("ti83plus-z80")
        || target.starts_with("ti84-z80")
        || target.starts_with("ti84plus-z80")
}

fn builtin_sdk_utf8(bytes: &'static [u8], module: &str) -> &'static str {
    std::str::from_utf8(bytes)
        .unwrap_or_else(|_| panic!("built-in SDK module `{module}` is not UTF-8"))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|candidate| candidate == &path) {
        paths.push(path);
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn direct_import_short_module_counts(program: &Program) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for declaration in &program.declarations {
        let Declaration::Import(import) = declaration else {
            continue;
        };
        let short_module = import.rsplit('.').next().unwrap_or(import);
        *counts.entry(short_module.to_owned()).or_insert(0) += 1;
    }
    counts
}

fn module_alias_declarations(
    import: &str,
    declarations: &[Declaration],
    include_short_aliases: bool,
) -> Vec<Declaration> {
    let Some(short_module) = import.rsplit('.').next() else {
        return Vec::new();
    };
    let mut prefixes = Vec::new();
    if include_short_aliases {
        prefixes.push(short_module.to_owned());
    }
    if short_module != import || !include_short_aliases {
        prefixes.push(import.to_owned());
    }
    declarations
        .iter()
        .flat_map(|declaration| {
            prefixes
                .iter()
                .filter_map(|prefix| alias_declaration(declaration, prefix))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn alias_declaration(declaration: &Declaration, prefix: &str) -> Option<Declaration> {
    match declaration {
        Declaration::Cfg {
            predicates,
            declaration,
        } => alias_declaration(declaration, prefix).map(|declaration| Declaration::Cfg {
            predicates: predicates.clone(),
            declaration: Box::new(declaration),
        }),
        Declaration::Bank { bank, declaration } => {
            alias_declaration(declaration, prefix).map(|declaration| Declaration::Bank {
                bank: *bank,
                declaration: Box::new(declaration),
            })
        }
        Declaration::Alias(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Alias(alias))
        }
        Declaration::Const(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Const(alias))
        }
        Declaration::Port(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Port(alias))
        }
        Declaration::Mmio(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Mmio(alias))
        }
        Declaration::Embed(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Embed(alias))
        }
        Declaration::Global(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Global(alias))
        }
        Declaration::Struct(decl) if decl.public => {
            let mut alias = decl.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Struct(alias))
        }
        Declaration::Function(function) if function.public && function.name != "main" => {
            let mut alias = function.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::Function(alias))
        }
        Declaration::ExternAsmFunction(function) if function.public => {
            let mut alias = function.clone();
            alias.name = format!("{prefix}.{}", alias.name);
            Some(Declaration::ExternAsmFunction(alias))
        }
        _ => None,
    }
}

fn validate_private_import_access(
    program: &Program,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
) -> Result<(), Diagnostic> {
    let mut private_imports = HashMap::new();
    let mut seen_imports = HashSet::new();
    for declaration in &program.declarations {
        let Declaration::Import(import) = declaration else {
            continue;
        };
        collect_private_imports(
            &program.source_path,
            import,
            sdk,
            source_overrides,
            &mut private_imports,
            &mut seen_imports,
        )?;
    }

    if private_imports.is_empty() {
        return Ok(());
    }

    for declaration in &program.declarations {
        validate_declaration_private_import_access(declaration, &private_imports)?;
    }
    Ok(())
}

fn collect_private_imports(
    source_path: &Path,
    import: &str,
    sdk: &SdkResolver,
    source_overrides: &HashMap<PathBuf, String>,
    private_imports: &mut HashMap<String, String>,
    seen: &mut HashSet<PathBuf>,
) -> Result<(), Diagnostic> {
    let (import_path, source) =
        read_import_source_with_overrides(source_path, import, sdk, source_overrides)?;
    let normalized = normalize_path(&import_path);
    if !seen.insert(normalized) {
        return Ok(());
    }
    let imported = parse_program(&import_path, &source)?;
    let short_module = import.rsplit('.').next().unwrap_or(import);
    for declaration in &imported.declarations {
        let Some(name) = declaration_name(declaration) else {
            continue;
        };
        if !declaration_is_public(declaration) {
            private_imports.insert(name.to_owned(), import.to_owned());
            private_imports.insert(format!("{short_module}.{name}"), import.to_owned());
            if short_module != import {
                private_imports.insert(format!("{import}.{name}"), import.to_owned());
            }
        }
    }
    for declaration in &imported.declarations {
        let Declaration::Import(nested) = declaration else {
            continue;
        };
        collect_private_imports(
            &import_path,
            nested,
            sdk,
            source_overrides,
            private_imports,
            seen,
        )?;
    }

    Ok(())
}

fn validate_declaration_private_import_access(
    declaration: &Declaration,
    private_imports: &HashMap<String, String>,
) -> Result<(), Diagnostic> {
    match declaration {
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            validate_declaration_private_import_access(declaration, private_imports)
        }
        Declaration::Const(decl) => {
            validate_type_private_import_access(&decl.ty, private_imports)?;
            validate_expr_private_import_access(&decl.value, private_imports, &HashSet::new())
        }
        Declaration::Alias(decl) => validate_type_private_import_access(&decl.ty, private_imports),
        Declaration::Port(decl) => {
            validate_type_private_import_access(&decl.ty, private_imports)?;
            validate_expr_private_import_access(&decl.value, private_imports, &HashSet::new())
        }
        Declaration::Mmio(decl) => {
            validate_type_private_import_access(&decl.ty, private_imports)?;
            validate_expr_private_import_access(&decl.value, private_imports, &HashSet::new())
        }
        Declaration::Embed(decl) => {
            validate_embed_source_private_import_access(
                &decl.source,
                private_imports,
                &HashSet::new(),
            )?;
            if let Some(align) = &decl.align {
                validate_expr_private_import_access(align, private_imports, &HashSet::new())?;
            }
            Ok(())
        }
        Declaration::Global(decl) => {
            validate_type_private_import_access(&decl.ty, private_imports)?;
            validate_expr_private_import_access(&decl.value, private_imports, &HashSet::new())
        }
        Declaration::Struct(decl) => {
            for field in &decl.fields {
                validate_type_private_import_access(&field.ty, private_imports)?;
            }
            Ok(())
        }
        Declaration::ExternAsmFunction(function) => {
            for param in &function.params {
                validate_type_private_import_access(&param.ty, private_imports)?;
            }
            if let Some(return_type) = &function.return_type {
                validate_type_private_import_access(return_type, private_imports)?;
            }
            Ok(())
        }
        Declaration::Function(function) => {
            validate_function_private_import_access(function, private_imports)
        }
        Declaration::Import(_) => Ok(()),
    }
}

fn validate_function_private_import_access(
    function: &Function,
    private_imports: &HashMap<String, String>,
) -> Result<(), Diagnostic> {
    let mut locals = function
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect::<HashSet<_>>();
    for param in &function.params {
        validate_type_private_import_access(&param.ty, private_imports)?;
    }
    if let Some(return_type) = &function.return_type {
        validate_type_private_import_access(return_type, private_imports)?;
    }
    for stmt in &function.body {
        validate_stmt_private_import_access(stmt, private_imports, &mut locals)?;
    }
    Ok(())
}

fn validate_stmt_private_import_access(
    stmt: &Stmt,
    private_imports: &HashMap<String, String>,
    locals: &mut HashSet<String>,
) -> Result<(), Diagnostic> {
    match stmt {
        Stmt::Let {
            name, ty, value, ..
        } => {
            validate_type_private_import_access(ty, private_imports)?;
            validate_expr_private_import_access(value, private_imports, locals)?;
            locals.insert(name.clone());
        }
        Stmt::Assign { target, value, .. } => {
            validate_place_private_import_access(target, private_imports, locals)?;
            validate_expr_private_import_access(value, private_imports, locals)?;
        }
        Stmt::If {
            condition,
            then_body,
            else_body,
        } => {
            validate_expr_private_import_access(condition, private_imports, locals)?;
            let mut then_locals = locals.clone();
            for stmt in then_body {
                validate_stmt_private_import_access(stmt, private_imports, &mut then_locals)?;
            }
            let mut else_locals = locals.clone();
            for stmt in else_body {
                validate_stmt_private_import_access(stmt, private_imports, &mut else_locals)?;
            }
        }
        Stmt::While { condition, body } => {
            validate_expr_private_import_access(condition, private_imports, locals)?;
            let mut body_locals = locals.clone();
            for stmt in body {
                validate_stmt_private_import_access(stmt, private_imports, &mut body_locals)?;
            }
        }
        Stmt::Loop { body } => {
            let mut body_locals = locals.clone();
            for stmt in body {
                validate_stmt_private_import_access(stmt, private_imports, &mut body_locals)?;
            }
        }
        Stmt::Return(Some(expr)) | Stmt::Expr(expr) => {
            validate_expr_private_import_access(expr, private_imports, locals)?;
        }
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => {}
        Stmt::Asm {
            inputs, outputs, ..
        } => {
            for input in inputs {
                validate_type_private_import_access(&input.ty, private_imports)?;
                reject_private_import_name(&input.name, private_imports, locals)?;
            }
            for output in outputs {
                validate_type_private_import_access(&output.ty, private_imports)?;
                reject_private_import_name(&output.name, private_imports, locals)?;
            }
        }
        Stmt::Out { port, value } => {
            reject_private_import_name(port, private_imports, locals)?;
            validate_expr_private_import_access(value, private_imports, locals)?;
        }
    }
    Ok(())
}

fn validate_embed_source_private_import_access(
    source: &EmbedSource,
    private_imports: &HashMap<String, String>,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match source {
        EmbedSource::File(_) | EmbedSource::Text(_) | EmbedSource::CStr(_) => Ok(()),
        EmbedSource::Bytes(values) => {
            for value in values {
                validate_expr_private_import_access(value, private_imports, locals)?;
            }
            Ok(())
        }
        EmbedSource::Repeat { value, len } => {
            validate_expr_private_import_access(value, private_imports, locals)?;
            validate_expr_private_import_access(len, private_imports, locals)
        }
    }
}

fn validate_place_private_import_access(
    place: &Place,
    private_imports: &HashMap<String, String>,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match place {
        Place::Ident(name) => reject_private_import_name(name, private_imports, locals),
        Place::Index { name, index } => {
            reject_private_import_name(name, private_imports, locals)?;
            validate_expr_private_import_access(index, private_imports, locals)
        }
        Place::Field { base, field } => {
            reject_private_import_name(&format!("{base}.{field}"), private_imports, locals)?;
            reject_private_import_name(base, private_imports, locals)
        }
        Place::Access(path) => validate_access_private_import_access(path, private_imports, locals),
        Place::Deref(expr) => validate_expr_private_import_access(expr, private_imports, locals),
    }
}

fn validate_expr_private_import_access(
    expr: &Expr,
    private_imports: &HashMap<String, String>,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    match expr {
        Expr::Ident(name) | Expr::AddressOf(name) | Expr::In(name) => {
            reject_private_import_name(name, private_imports, locals)
        }
        Expr::Index { name, index } | Expr::AddressOfIndex { name, index } => {
            reject_private_import_name(name, private_imports, locals)?;
            validate_expr_private_import_access(index, private_imports, locals)
        }
        Expr::Field { base, field } | Expr::AddressOfField { base, field } => {
            reject_private_import_name(&format!("{base}.{field}"), private_imports, locals)?;
            reject_private_import_name(base, private_imports, locals)
        }
        Expr::Access(path) | Expr::AddressOfAccess(path) => {
            validate_access_private_import_access(path, private_imports, locals)
        }
        Expr::Cast { expr, ty } => {
            validate_type_private_import_access(ty, private_imports)?;
            validate_expr_private_import_access(expr, private_imports, locals)
        }
        Expr::Unary { expr, .. }
        | Expr::Deref(expr)
        | Expr::BankedPointer { pointer: expr, .. } => {
            validate_expr_private_import_access(expr, private_imports, locals)
        }
        Expr::Binary { left, right, .. } => {
            validate_expr_private_import_access(left, private_imports, locals)?;
            validate_expr_private_import_access(right, private_imports, locals)
        }
        Expr::Array(values) => {
            for value in values {
                validate_expr_private_import_access(value, private_imports, locals)?;
            }
            Ok(())
        }
        Expr::StructInit { ty, fields } => {
            reject_private_import_type_name(ty, private_imports)?;
            for (_, value) in fields {
                validate_expr_private_import_access(value, private_imports, locals)?;
            }
            Ok(())
        }
        Expr::Call { path, args } => {
            if let Some(name) = path.first() {
                reject_private_import_name(name, private_imports, locals)?;
            }
            for arg in args {
                validate_expr_private_import_access(arg, private_imports, locals)?;
            }
            Ok(())
        }
        Expr::TypedInt(_, ty) => validate_type_private_import_access(ty, private_imports),
        Expr::Int(_) | Expr::Char(_) | Expr::Bool(_) | Expr::String(_) => Ok(()),
    }
}

fn validate_access_private_import_access(
    path: &AccessPath,
    private_imports: &HashMap<String, String>,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    reject_private_import_name(&path.root, private_imports, locals)?;
    for segment in &path.segments {
        if let AccessSegment::Index(index) = segment {
            validate_expr_private_import_access(index, private_imports, locals)?;
        }
    }
    Ok(())
}

fn validate_type_private_import_access(
    ty: &Type,
    private_imports: &HashMap<String, String>,
) -> Result<(), Diagnostic> {
    match ty {
        Type::Named(name) => reject_private_import_type_name(name, private_imports),
        Type::Ptr(inner) => validate_type_private_import_access(inner, private_imports),
        Type::Array { element, len } => {
            validate_type_private_import_access(element, private_imports)?;
            validate_expr_private_import_access(len, private_imports, &HashSet::new())
        }
    }
}

fn reject_private_import_type_name(
    name: &str,
    private_imports: &HashMap<String, String>,
) -> Result<(), Diagnostic> {
    if let Some(import) = private_imports.get(name) {
        return Err(Diagnostic::new(format!(
            "declaration `{name}` from import `{import}` is private"
        )));
    }
    Ok(())
}

fn reject_private_import_name(
    name: &str,
    private_imports: &HashMap<String, String>,
    locals: &HashSet<String>,
) -> Result<(), Diagnostic> {
    if locals.contains(name) {
        return Ok(());
    }
    if let Some(import) = private_imports.get(name) {
        return Err(Diagnostic::new(format!(
            "declaration `{name}` from import `{import}` is private"
        )));
    }
    Ok(())
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

fn declaration_is_public(declaration: &Declaration) -> bool {
    match declaration {
        Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
            declaration_is_public(declaration)
        }
        Declaration::Import(_) => true,
        Declaration::Const(decl) => decl.public,
        Declaration::Alias(decl) => decl.public,
        Declaration::Port(decl) => decl.public,
        Declaration::Mmio(decl) => decl.public,
        Declaration::Embed(decl) => decl.public,
        Declaration::Global(decl) => decl.public,
        Declaration::Struct(decl) => decl.public,
        Declaration::ExternAsmFunction(decl) => decl.public,
        Declaration::Function(decl) => decl.public,
    }
}

#[cfg(all(test, feature = "i8086"))]
mod i8086_validation_tests {
    use super::*;

    #[test]
    fn check_rejects_post_8086_inline_assembly() {
        let error = check_source_with_sdk(
            "fn main() { asm volatile { \"pusha\" } }",
            &CompileOptions {
                source: PathBuf::from("main.ezra"),
                debug_comments: false,
                default_sdk_symbols: true,
            },
            &SdkResolver {
                target: Some("bare-i8086".to_owned()),
                sdk_roots: Vec::new(),
            },
        )
        .unwrap_err();

        assert!(
            error
                .message
                .contains("assembler does not support 8086 instruction `pusha`"),
            "{error}"
        );
    }

    #[test]
    fn msdos_builtin_sdk_modules_resolve_at_canonical_toolchain_paths() {
        const MODULES: &[&str] = &[
            "dos.constants",
            "dos.raw",
            "dos.console",
            "dos.file",
            "dos.directory",
            "dos.memory",
            "dos.datetime",
            "dos.process",
            "dos.psp",
        ];
        let sdk = SdkResolver {
            target: Some("msdos-com-i8086".to_owned()),
            sdk_roots: Vec::new(),
        };

        assert_eq!(builtin_sdk_modules(sdk.target.as_deref()), MODULES);
        for module in MODULES {
            let (path, source) =
                resolve_import_source(Path::new("src/main.ezra"), module, &sdk).unwrap();
            assert_eq!(
                path,
                PathBuf::from(format!(
                    "toolchains/msdos-i8086/sdk/{}.ezra",
                    module.replace('.', "/")
                ))
            );
            assert!(!source.is_empty());

            check_source_with_sdk(
                &format!("import {module}\nfn main() {{}}\n"),
                &CompileOptions {
                    source: PathBuf::from("src/main.ezra"),
                    debug_comments: false,
                    default_sdk_symbols: true,
                },
                &sdk,
            )
            .unwrap_or_else(|error| panic!("failed to compile `{module}`: {error}"));
        }
    }

    #[test]
    fn msdos_audited_sdk_paths_compile_through_the_strict_8086_assembler() {
        const PROGRAMS: &[&str] = &[
            r#"
                import dos.console
                fn main() {
                    console.direct_io(console.DIRECT_INPUT)
                    console.flush_and_read_char(1)
                    console.flush_and_direct_input()
                    console.flush_and_read_buffered(cast<ptr<u8>>(0xA100))
                    console.direct_ready()
                }
            "#,
            r#"
                import dos.file
                fn main() {
                    let buffer: ptr<u8> = cast<ptr<u8>>(0xA100)
                    file.extended_open("TEST.TXT", 0, 0, 0)
                    file.read_exact(0, buffer, 1)
                    file.write_all(1, buffer, 1)
                    file.ioctl_device_info(0)
                    file.ioctl_input_ready(0)
                    file.ioctl_output_ready(1)
                }
            "#,
            r#"
                import dos.memory
                fn main() {
                    memory.read_byte(0x1000, 0)
                    memory.write_byte(0x1000, 0, 1)
                    memory.read_word(0x1000, 0)
                    memory.write_word(0x1000, 0, 1)
                    memory.copy_to_segment(0x1000, 0, "x", 1)
                    memory.copy_from_segment(cast<ptr<u8>>(0xA100), 0x1000, 0, 1)
                }
            "#,
            r#"
                import dos.raw
                fn main() {
                    raw.refresh_extended_error()
                    raw.extended_error_code()
                    raw.extended_error_class()
                    raw.extended_error_action()
                    raw.extended_error_locus()
                }
            "#,
            r#"
                import dos.psp
                fn main() {
                    psp.saved_stack_offset()
                    psp.saved_stack_segment()
                    psp.previous_psp_offset()
                    psp.previous_psp_segment()
                    psp.dos_version_raw()
                    psp.dos_version()
                    psp.default_dta()
                    psp.job_file_table_size()
                    psp.job_file_table_offset()
                    psp.job_file_table_segment()
                    psp.job_file_table_entry(0)
                }
            "#,
            r#"
                import dos.psp
                fn main() {
                    psp.has_environment()
                    psp.environment_byte(0)
                    psp.environment_trailer_offset()
                    psp.environment_program_path_offset()
                    psp.fcb_file_size_low(psp.fcb1())
                    psp.fcb_file_size_high(psp.fcb1())
                    psp.fcb_set_random_record(psp.fcb1(), 1, 2)
                    psp.fcb_random_record_low(psp.fcb1())
                    psp.fcb_random_record_high(psp.fcb1())
                }
            "#,
            r#"
                import dos.directory
                fn main() {
                    directory.make_directory("TESTDIR")
                    directory.remove_directory("TESTDIR")
                    directory.change_directory(".")
                    directory.get_current_directory(cast<ptr<u8>>(0xA100), 0)
                    directory.free_space(0)
                }
            "#,
            r#"
                import dos.process
                fn main() {
                    process.version()
                    process.exec(0, "CHILD.COM", cast<ptr<u8>>(0xA100))
                    process.set_vector(0x60, 0x0100)
                    process.set_verify(false)
                }
            "#,
        ];
        let sdk = SdkResolver {
            target: Some("msdos-com-i8086".to_owned()),
            sdk_roots: Vec::new(),
        };

        for source in PROGRAMS {
            check_source_with_sdk(
                source,
                &CompileOptions {
                    source: PathBuf::from("src/main.ezra"),
                    debug_comments: false,
                    default_sdk_symbols: true,
                },
                &sdk,
            )
            .unwrap_or_else(|error| panic!("failed DOS SDK strict smoke program: {error}"));
        }
    }

    #[test]
    fn check_uses_a_16_bit_layout_for_arbitrary_i8086_targets() {
        check_source_with_sdk(
            "fn main() {}",
            &CompileOptions {
                source: PathBuf::from("main.ezra"),
                debug_comments: false,
                default_sdk_symbols: true,
            },
            &SdkResolver {
                target: Some("custom-board-i8086".to_owned()),
                sdk_roots: Vec::new(),
            },
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests;
