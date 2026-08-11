//! Alloc-only in-process compiler API backed exclusively by a virtual workspace.

use crate::compat::prelude::*;

pub use crate::workspace::{Workspace, WorkspaceFile};

#[cfg(feature = "avr")]
use crate::asm::emit_avr_assembly_with_options;
#[cfg(feature = "i8086")]
use crate::asm::emit_i8086_assembly_with_options;
#[cfg(feature = "mos6502")]
use crate::asm::emit_mos6502_assembly_with_options;
#[cfg(feature = "msp430")]
use crate::asm::emit_msp430_assembly_with_options;
#[cfg(feature = "pic18")]
use crate::asm::emit_pic18_assembly_with_options;

use crate::{
    asm::{AssemblyOptions, AssemblyProgram, emit_ez80_assembly_with_options},
    ast::{CfgPredicate, Declaration, EmbedSource, Expr, Program},
    diagnostic::Diagnostic,
    layout::{Layout, default_layout_for_target},
    package::{PackageContext, PackageRequest, package_executable_with_context},
    parser::parse_program,
    target::{
        Address24, AssemblerCpu, CpuFamily, DEFAULT_TARGET_TRIPLE, OutputFormat, TargetProfile,
        is_msdos_i8086_target, memory_model_for_cpu, resolve_target_profile,
    },
    vm::{
        AssemblerSourceOptions, AssemblySymbol, assemble_program_with_options_at,
        assemble_subset_with_options_at, assemble_subset_with_symbols_at,
    },
    workspace::{materialize_workspace_embeds, normalize_virtual_path},
};

/// Options for compiling virtual Ezra source without host services.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileRequest {
    /// Logical path used for diagnostics and relative import resolution.
    pub source_path: String,
    /// Target triple used for validation, code generation, and packaging.
    pub target: String,
    /// Retained for API parity. No-std builds never consult these host paths.
    pub sdk_paths: Vec<String>,
    /// Include generator debug comments where supported.
    pub debug_comments: bool,
    /// Enable target SDK symbols built into the code generator.
    pub default_sdk_symbols: bool,
    /// Optimization level and per-pass overrides.
    pub optimization: crate::optimization::OptimizationOptions,
}

impl CompileRequest {
    pub fn new(source_path: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            source_path: source_path.into(),
            target: target.into(),
            sdk_paths: Vec::new(),
            debug_comments: false,
            default_sdk_symbols: true,
            optimization: crate::optimization::OptimizationOptions::default(),
        }
    }

    pub fn with_default_target(source_path: impl Into<String>) -> Self {
        Self::new(source_path, DEFAULT_TARGET_TRIPLE)
    }
}

/// Semantic/import summary for a virtual source build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileReport {
    pub imports: usize,
    pub declarations: usize,
    pub has_main: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyCompilation {
    pub report: CompileReport,
    pub program: Program,
    pub assembly: String,
}

/// Resolved build configuration for filesystem-free alloc-only consumers.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildRequest {
    pub target: TargetProfile,
    pub output_format: OutputFormat,
    pub assembler_cpu: AssemblerCpu,
    pub layout: Layout,
    pub executable_name: Option<String>,
    pub package_context: PackageContext,
}

impl BuildRequest {
    pub fn for_target(target: impl AsRef<str>) -> Result<Self, Diagnostic> {
        let target = resolve_target_profile(Some(target.as_ref())).map_err(Diagnostic::new)?;
        let layout = layout_for_target(&target.triple.value, target.triple.cpu);
        validate_layout_for_cpu(&layout, target.triple.cpu, &target.triple.value)?;
        Ok(Self {
            output_format: target.output_format,
            assembler_cpu: AssemblerCpu::from(target.triple.cpu),
            target,
            layout,
            executable_name: None,
            package_context: PackageContext::new(),
        })
    }

    fn package_request(&self) -> PackageRequest {
        PackageRequest {
            target: self.target.triple.value.clone(),
            output_format: self.output_format,
            load_addr: self.layout.load.get(),
            entry_addr: self.layout.entry.get(),
            executable_name: self.executable_name.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkInput {
    Generated,
    Assembly,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinkedCompilation {
    pub machine_code: Vec<u8>,
    pub map: String,
    pub symbols: Vec<AssemblySymbol>,
    pub executable: Vec<u8>,
    pub output_format: OutputFormat,
    pub executable_extension: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuildCompilation {
    pub report: CompileReport,
    pub program: Program,
    pub assembly: String,
    pub machine_code: Vec<u8>,
    pub map: String,
    pub symbols: Vec<AssemblySymbol>,
    pub executable: Vec<u8>,
    pub output_format: OutputFormat,
    pub executable_extension: &'static str,
}

/// Compile one source string without imports.
pub fn compile_source_to_assembly(
    source: &str,
    request: &CompileRequest,
) -> Result<AssemblyCompilation, Diagnostic> {
    let path = normalize_virtual_path(&request.source_path);
    let files = [WorkspaceFile::text(&path, source)];
    compile_workspace_to_assembly(&Workspace::new(&files), &path, request)
}

/// Parse and compile a root file whose imports are resolved only from `workspace`.
pub fn compile_workspace_to_assembly(
    workspace: &Workspace<'_>,
    root: &str,
    request: &CompileRequest,
) -> Result<AssemblyCompilation, Diagnostic> {
    let target = resolve_target_profile(Some(&request.target)).map_err(Diagnostic::new)?;
    let layout = layout_for_target(&request.target, target.triple.cpu);
    validate_layout_for_cpu(&layout, target.triple.cpu, &request.target)?;
    compile_workspace_to_assembly_with_resolved_request(workspace, root, request, &target, &layout)
}

/// Compile a virtual workspace using an explicit target layout and code-generation settings.
pub fn compile_workspace_to_assembly_with_request(
    workspace: &Workspace<'_>,
    root: &str,
    request: &CompileRequest,
    build: &BuildRequest,
) -> Result<AssemblyCompilation, Diagnostic> {
    if request.target != build.target.triple.value {
        return Err(Diagnostic::new(format!(
            "compile target `{}` does not match build target `{}`",
            request.target, build.target.triple.value
        )));
    }
    validate_layout_for_cpu(
        &build.layout,
        build.target.triple.cpu,
        &build.target.triple.value,
    )?;
    compile_workspace_to_assembly_with_resolved_request(
        workspace,
        root,
        request,
        &build.target,
        &build.layout,
    )
}

fn compile_workspace_to_assembly_with_resolved_request(
    workspace: &Workspace<'_>,
    root: &str,
    request: &CompileRequest,
    target: &TargetProfile,
    layout: &Layout,
) -> Result<AssemblyCompilation, Diagnostic> {
    if !matches!(
        target.triple.cpu,
        CpuFamily::Ez80
            | CpuFamily::Z80
            | CpuFamily::R800
            | CpuFamily::Z80N
            | CpuFamily::Z180
            | CpuFamily::I8080
            | CpuFamily::I8085
            | CpuFamily::I8086
            | CpuFamily::Avr
            | CpuFamily::Pic18
            | CpuFamily::Mos6502
            | CpuFamily::Cmos65C02
            | CpuFamily::Wdc65C816
            | CpuFamily::Ricoh2A03
    ) {
        return Err(Diagnostic::new(format!(
            "no-std source code generation is currently available only for eZ80/Z80/R800, i8086, AVR, PIC18, and MOS 6502-family targets, not `{}`",
            target.triple.cpu.as_str()
        )));
    }

    let root = normalize_virtual_path(root);
    let source = workspace_text(workspace, &root)?;
    let root_program = parse_program(&root, source)?;
    let imports = root_program
        .declarations
        .iter()
        .filter(|declaration| matches!(declaration, Declaration::Import(_)))
        .count();
    let mut stack = Vec::new();
    let mut seen = HashSet::new();
    let program = resolve_program(workspace, root_program, request, &mut stack, &mut seen)?;
    let has_main = program.main_function().is_some();
    if !has_main {
        return Err(Diagnostic::new("missing required `fn main()`"));
    }
    let main = program.main_function().expect("main presence checked");
    if !main.params.is_empty() {
        return Err(Diagnostic::new("main function cannot take parameters"));
    }
    if main.return_type.is_some() || main.second_return_type.is_some() {
        return Err(Diagnostic::new("main function cannot return a value"));
    }

    let report = CompileReport {
        imports,
        declarations: program.declarations.len(),
        has_main,
    };
    let mut options = assembly_options_for_layout(
        layout,
        target.triple.cpu,
        &request.target,
        request.debug_comments,
        request.default_sdk_symbols,
    );
    options.optimization = request.optimization.clone();
    let assembly = match target.triple.cpu {
        CpuFamily::I8086 => {
            #[cfg(feature = "i8086")]
            {
                emit_i8086_assembly_with_options(&program, options)?
            }
            #[cfg(not(feature = "i8086"))]
            {
                return Err(Diagnostic::new(
                    "i8086 source compilation requires the `i8086` Cargo feature",
                ));
            }
        }
        CpuFamily::Avr => {
            #[cfg(feature = "avr")]
            {
                emit_avr_assembly_with_options(&program, options)?
            }
            #[cfg(not(feature = "avr"))]
            {
                return Err(Diagnostic::new(
                    "AVR source compilation requires the `avr` Cargo feature",
                ));
            }
        }
        CpuFamily::Pic18 => {
            #[cfg(feature = "pic18")]
            {
                emit_pic18_assembly_with_options(&program, options)?
            }
            #[cfg(not(feature = "pic18"))]
            {
                return Err(Diagnostic::new(
                    "PIC18 source compilation requires the `pic18` Cargo feature",
                ));
            }
        }
        CpuFamily::Mos6502 | CpuFamily::Cmos65C02 | CpuFamily::Wdc65C816 | CpuFamily::Ricoh2A03 => {
            #[cfg(feature = "mos6502")]
            {
                emit_mos6502_assembly_with_options(&program, options)?
            }
            #[cfg(not(feature = "mos6502"))]
            {
                return Err(Diagnostic::new(
                    "MOS 6502 source compilation requires the `mos6502` Cargo feature",
                ));
            }
        }
        CpuFamily::Msp430 | CpuFamily::Msp430X | CpuFamily::Msp430X2 => {
            #[cfg(feature = "msp430")]
            {
                emit_msp430_assembly_with_options(&program, options)?
            }
            #[cfg(not(feature = "msp430"))]
            {
                return Err(Diagnostic::new(
                    "MSP430 source compilation requires the `msp430` Cargo feature",
                ));
            }
        }
        _ => emit_ez80_assembly_with_options(&program, options)?,
    };
    validate_generated_assembly(&assembly, target.triple.cpu, layout)?;
    Ok(AssemblyCompilation {
        report,
        program,
        assembly,
    })
}

/// Compile, assemble, and package a virtual workspace without host I/O.
pub fn build_workspace(
    workspace: &Workspace<'_>,
    root: &str,
    request: &CompileRequest,
) -> Result<BuildCompilation, Diagnostic> {
    let mut build = BuildRequest::for_target(&request.target)?;
    build.executable_name = normalize_virtual_path(root)
        .rsplit('/')
        .next()
        .and_then(|name| name.split('.').next())
        .map(str::to_owned);
    build_workspace_with_request(workspace, root, request, &build)
}

/// Compile, assemble, and package a virtual workspace with explicit build settings.
pub fn build_workspace_with_request(
    workspace: &Workspace<'_>,
    root: &str,
    request: &CompileRequest,
    build: &BuildRequest,
) -> Result<BuildCompilation, Diagnostic> {
    if request.target != build.target.triple.value {
        return Err(Diagnostic::new(format!(
            "compile target `{}` does not match build target `{}`",
            request.target, build.target.triple.value
        )));
    }
    validate_layout_for_cpu(
        &build.layout,
        build.target.triple.cpu,
        &build.target.triple.value,
    )?;
    let compilation = compile_workspace_to_assembly_with_request(workspace, root, request, build)?;
    let root = normalize_virtual_path(root);
    let linked =
        link_generated_assembly(&root, &compilation.assembly, &compilation.program, build)?;

    Ok(BuildCompilation {
        report: compilation.report,
        program: compilation.program,
        assembly: compilation.assembly,
        machine_code: linked.machine_code,
        map: linked.map,
        symbols: linked.symbols,
        executable: linked.executable,
        output_format: linked.output_format,
        executable_extension: linked.executable_extension,
    })
}

/// Link generated assembly and package it with caller-owned build settings.
pub fn link_generated_assembly(
    source_path: &str,
    assembly: &str,
    program: &Program,
    build: &BuildRequest,
) -> Result<LinkedCompilation, Diagnostic> {
    validate_layout_for_cpu(
        &build.layout,
        build.target.triple.cpu,
        &build.target.triple.value,
    )?;
    let assembled = assemble_subset_with_options_at(
        build.assembler_cpu,
        assembly,
        build.layout.entry.get(),
        &assembly_source_options(source_path, &build.layout),
    )?;
    validate_text_section_fit(&build.layout, assembled_text_len(&assembled))?;
    let map = flat_assembly_map_at(
        assembled.bytes.len(),
        &assembled.symbols,
        build.layout.entry.get(),
    )?;
    if build.target.triple.value.starts_with("nes-") {
        let mut build = build.clone();
        build.package_context.nes = Some(crate::package::NesPackageOptions {
            chr_payload: collect_nes_chr_assets(program)?,
        });
        package_linked(&build, assembled.bytes, map, assembled.symbols)
    } else {
        package_linked(build, assembled.bytes, map, assembled.symbols)
    }
}

/// Link a preprocessed standalone assembly program at the layout load address.
pub fn link_assembly_program(
    source_path: &str,
    program: &AssemblyProgram,
    build: &BuildRequest,
) -> Result<LinkedCompilation, Diagnostic> {
    link_assembly_program_at(source_path, program, build.layout.load.get(), build)
}

/// Link a preprocessed standalone assembly program at an explicit base address.
pub fn link_assembly_program_at(
    source_path: &str,
    program: &AssemblyProgram,
    base_addr: u32,
    build: &BuildRequest,
) -> Result<LinkedCompilation, Diagnostic> {
    validate_layout_for_cpu(
        &build.layout,
        build.target.triple.cpu,
        &build.target.triple.value,
    )?;
    let max_addr = if build.target.memory.address_width_bits >= 24 {
        Address24::MAX
    } else {
        (1u32 << build.target.memory.address_width_bits) - 1
    };
    if base_addr > max_addr {
        return Err(Diagnostic::new(format!(
            "base address 0x{base_addr:X} is outside the {}-bit address space for target `{}`",
            build.target.memory.address_width_bits, build.target.triple.value
        )));
    }
    if build.output_format == OutputFormat::GameBoyGb && base_addr != 0x0150 {
        return Err(Diagnostic::new(
            "Game Boy assembly must use base address 0x0150",
        ));
    }
    let assembled = assemble_program_with_options_at(
        build.assembler_cpu,
        program,
        base_addr,
        &assembly_source_options(source_path, &build.layout),
    )?;
    let map = flat_assembly_map_at(assembled.bytes.len(), &assembled.symbols, base_addr)?;
    package_linked(build, assembled.bytes, map, assembled.symbols)
}

fn collect_nes_chr_assets(program: &Program) -> Result<Vec<u8>, Diagnostic> {
    fn visit(declarations: &[Declaration], output: &mut Vec<u8>) -> Result<(), Diagnostic> {
        for declaration in declarations {
            match declaration {
                Declaration::Cfg { declaration, .. } | Declaration::Bank { declaration, .. } => {
                    visit(core::slice::from_ref(declaration.as_ref()), output)?;
                }
                Declaration::Embed(embed)
                    if embed.section.as_deref().unwrap_or(".assets") == ".assets" =>
                {
                    let align = match embed.align.as_ref() {
                        None => 1usize,
                        Some(Expr::Int(value)) if *value > 0 && (*value & (*value - 1)) == 0 => {
                            usize::try_from(*value).map_err(|_| {
                                Diagnostic::new("NES CHR asset alignment exceeds host range")
                            })?
                        }
                        Some(_) => {
                            return Err(Diagnostic::new(format!(
                                "NES CHR asset `{}` alignment must be a constant power of two",
                                embed.name
                            )));
                        }
                    };
                    let bytes = match &embed.source {
                        EmbedSource::Bytes(values) => values
                            .iter()
                            .map(|value| match value {
                                Expr::Int(value) => u8::try_from(*value).map_err(|_| {
                                    Diagnostic::new(format!(
                                        "NES CHR asset `{}` contains a byte outside u8 range",
                                        embed.name
                                    ))
                                }),
                                _ => Err(Diagnostic::new(format!(
                                    "NES CHR asset `{}` must contain materialized constant bytes",
                                    embed.name
                                ))),
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                        EmbedSource::Text(text) => text.as_bytes().to_vec(),
                        EmbedSource::CStr(text) => {
                            let mut bytes = text.as_bytes().to_vec();
                            bytes.push(0);
                            bytes
                        }
                        EmbedSource::Repeat {
                            value: Expr::Int(value),
                            len: Expr::Int(len),
                        } => {
                            let value = u8::try_from(*value).map_err(|_| {
                                Diagnostic::new(format!(
                                    "NES CHR asset `{}` repeat byte is outside u8 range",
                                    embed.name
                                ))
                            })?;
                            let len = usize::try_from(*len).map_err(|_| {
                                Diagnostic::new(format!(
                                    "NES CHR asset `{}` repeat length is invalid",
                                    embed.name
                                ))
                            })?;
                            vec![value; len]
                        }
                        _ => {
                            return Err(Diagnostic::new(format!(
                                "NES CHR asset `{}` must be materialized before packaging",
                                embed.name
                            )));
                        }
                    };
                    if bytes.len() % 16 != 0 {
                        return Err(Diagnostic::new(format!(
                            "NES CHR asset `{}` must contain whole 16-byte tiles, got {} bytes",
                            embed.name,
                            bytes.len()
                        )));
                    }
                    let aligned = output
                        .len()
                        .checked_add(align - 1)
                        .map(|value| value & !(align - 1))
                        .ok_or_else(|| Diagnostic::new("NES CHR assets exceed host range"))?;
                    output.resize(aligned, 0);
                    output.extend_from_slice(&bytes);
                }
                _ => {}
            }
        }
        Ok(())
    }

    let mut output = Vec::new();
    visit(&program.declarations, &mut output)?;
    Ok(output)
}

fn package_linked(
    build: &BuildRequest,
    machine_code: Vec<u8>,
    map: String,
    symbols: Vec<AssemblySymbol>,
) -> Result<LinkedCompilation, Diagnostic> {
    let executable = package_executable_with_context(
        &build.package_request(),
        &build.package_context,
        &machine_code,
    )
    .map_err(|error| Diagnostic::new(error.message))?;
    Ok(LinkedCompilation {
        machine_code,
        map,
        symbols,
        executable,
        output_format: build.output_format,
        executable_extension: if build.target.triple.value.starts_with("gameboy-color-") {
            "gbc"
        } else {
            build.output_format.extension()
        },
    })
}

fn workspace_text<'a>(workspace: &Workspace<'a>, path: &str) -> Result<&'a str, Diagnostic> {
    let bytes = workspace
        .file(path)
        .ok_or_else(|| Diagnostic::new(format!("workspace does not contain source `{path}`")))?;
    core::str::from_utf8(bytes)
        .map_err(|_| Diagnostic::new(format!("workspace source `{path}` is not UTF-8")))
}

fn resolve_program(
    workspace: &Workspace<'_>,
    mut program: Program,
    request: &CompileRequest,
    stack: &mut Vec<String>,
    seen: &mut HashSet<String>,
) -> Result<Program, Diagnostic> {
    let path = normalize_virtual_path(&program.source_path);
    if stack.contains(&path) {
        let mut cycle = stack.clone();
        cycle.push(path);
        return Err(Diagnostic::new(format!(
            "cyclic import detected: {}",
            cycle.join(" -> ")
        )));
    }
    if !seen.insert(path.clone()) {
        program.declarations.clear();
        program.source_units.clear();
        return Ok(program);
    }

    program.declarations = active_declarations(program.declarations, request)?;
    materialize_workspace_embeds(&mut program, workspace)?;
    let short_counts = direct_import_short_module_counts(&program);
    stack.push(path.clone());
    let mut declarations = Vec::new();
    let mut source_units = Vec::new();

    for declaration in &program.declarations {
        let Declaration::Import(import) = declaration else {
            continue;
        };
        let import_path = resolve_import_path(workspace, &path, import).ok_or_else(|| {
            Diagnostic::new(format!(
                "failed to resolve import `{import}` from `{path}` in virtual workspace"
            ))
        })?;
        if seen.contains(&import_path) && !stack.contains(&import_path) {
            continue;
        }
        let source = workspace_text(workspace, &import_path)?;
        let mut imported = parse_program(&import_path, source)?;
        imported.declarations = active_declarations(imported.declarations, request)?;
        materialize_workspace_embeds(&mut imported, workspace)?;
        let short = import.rsplit('.').next().unwrap_or(import);
        let aliases = module_alias_declarations(
            import,
            &imported.declarations,
            short_counts.get(short).copied().unwrap_or_default() <= 1,
        );
        let imported = resolve_program(workspace, imported, request, stack, seen)?;
        source_units.extend(imported.source_units.iter().cloned());
        declarations.extend(
            imported
                .declarations
                .into_iter()
                .filter(|declaration| !is_entry_function(declaration)),
        );
        declarations.extend(aliases);
    }
    stack.pop();
    declarations.extend(
        program
            .declarations
            .into_iter()
            .filter(|declaration| !matches!(declaration, Declaration::Import(_))),
    );
    source_units.extend(program.source_units);
    program.declarations = declarations;
    program.source_units = source_units;
    Ok(program)
}

fn resolve_import_path(workspace: &Workspace<'_>, importer: &str, import: &str) -> Option<String> {
    let module = format!("{}.ezra", import.replace('.', "/"));
    let mut directory = importer
        .rsplit_once('/')
        .map(|(directory, _)| directory)
        .unwrap_or("");
    loop {
        let candidate = if directory.is_empty() {
            module.clone()
        } else {
            format!("{directory}/{module}")
        };
        let candidate = normalize_virtual_path(&candidate);
        if workspace.file(&candidate).is_some() {
            return Some(candidate);
        }
        let Some((parent, _)) = directory.rsplit_once('/') else {
            if !directory.is_empty() {
                directory = "";
                continue;
            }
            break;
        };
        directory = parent;
    }
    None
}

fn direct_import_short_module_counts(program: &Program) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for declaration in &program.declarations {
        if let Declaration::Import(import) = declaration {
            let short = import.rsplit('.').next().unwrap_or(import);
            *counts.entry(short.to_owned()).or_insert(0) += 1;
        }
    }
    counts
}

fn module_alias_declarations(
    import: &str,
    declarations: &[Declaration],
    include_short_aliases: bool,
) -> Vec<Declaration> {
    let short = import.rsplit('.').next().unwrap_or(import);
    let mut prefixes = Vec::new();
    if include_short_aliases {
        prefixes.push(short.to_owned());
    }
    if short != import || !include_short_aliases {
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
    let qualified = |name: &str| format!("{prefix}.{name}");
    match declaration {
        Declaration::Alias(value) if value.public => {
            let mut value = value.clone();
            value.name = qualified(&value.name);
            Some(Declaration::Alias(value))
        }
        Declaration::Const(value) if value.public => {
            let mut value = value.clone();
            value.name = qualified(&value.name);
            Some(Declaration::Const(value))
        }
        Declaration::Port(value) if value.public => {
            let mut value = value.clone();
            value.name = qualified(&value.name);
            Some(Declaration::Port(value))
        }
        Declaration::Mmio(value) if value.public => {
            let mut value = value.clone();
            value.name = qualified(&value.name);
            Some(Declaration::Mmio(value))
        }
        Declaration::Embed(value) if value.public => {
            let mut value = value.clone();
            value.name = qualified(&value.name);
            Some(Declaration::Embed(value))
        }
        Declaration::Global(value) if value.public => {
            let mut value = value.clone();
            value.name = qualified(&value.name);
            Some(Declaration::Global(value))
        }
        Declaration::Struct(value) if value.public => {
            let mut value = value.clone();
            value.name = qualified(&value.name);
            Some(Declaration::Struct(value))
        }
        Declaration::Function(value) if value.public && value.name != "main" => {
            let mut value = value.clone();
            value.name = qualified(&value.name);
            Some(Declaration::Function(value))
        }
        Declaration::ExternAsmFunction(value) if value.public => {
            let mut value = value.clone();
            value.name = qualified(&value.name);
            Some(Declaration::ExternAsmFunction(value))
        }
        _ => None,
    }
}

fn is_entry_function(declaration: &Declaration) -> bool {
    matches!(declaration, Declaration::Function(function) if function.name == "main")
}

fn active_declarations(
    declarations: Vec<Declaration>,
    request: &CompileRequest,
) -> Result<Vec<Declaration>, Diagnostic> {
    declarations
        .into_iter()
        .filter_map(|declaration| active_declaration(declaration, request).transpose())
        .collect()
}

fn active_declaration(
    declaration: Declaration,
    request: &CompileRequest,
) -> Result<Option<Declaration>, Diagnostic> {
    match declaration {
        Declaration::Cfg {
            predicates,
            declaration,
        } => {
            for predicate in &predicates {
                if !cfg_matches(predicate, request)? {
                    return Ok(None);
                }
            }
            active_declaration(*declaration, request)
        }
        declaration => Ok(Some(declaration)),
    }
}

fn cfg_matches(predicate: &CfgPredicate, request: &CompileRequest) -> Result<bool, Diagnostic> {
    let triple = crate::target::parse_target_triple(&request.target).map_err(Diagnostic::new)?;
    let parts = request.target.split('-').collect::<Vec<_>>();
    let memory = memory_model_for_cpu(triple.cpu)
        .ok_or_else(|| Diagnostic::new("target has no memory model"))?;
    match predicate {
        CfgPredicate::Target(value) => Ok(request.target == *value),
        CfgPredicate::TargetFamily(value) => Ok(parts.first().copied() == Some(value.as_str())),
        CfgPredicate::Cpu(value) => Ok(triple.cpu.as_str() == value),
        CfgPredicate::Vendor(value) => Ok(parts.get(1).copied() == Some(value.as_str())),
        CfgPredicate::Os(value) => Ok(parts.iter().any(|part| part == value)),
        CfgPredicate::PointerWidth(value) => Ok(memory.pointer_width_bits == *value),
        CfgPredicate::AddressWidth(value) => Ok(memory.address_width_bits == *value),
        CfgPredicate::Feature(value) => Ok(parts.iter().any(|part| part == value)),
        CfgPredicate::Debug => Ok(cfg!(debug_assertions)),
        CfgPredicate::Release => Ok(!cfg!(debug_assertions)),
        CfgPredicate::All(values) => {
            for value in values {
                if !cfg_matches(value, request)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        CfgPredicate::Any(values) => {
            for value in values {
                if cfg_matches(value, request)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        CfgPredicate::Not(value) => Ok(!cfg_matches(value, request)?),
    }
}

pub fn assembly_options_for_target(
    target: &str,
    cpu: CpuFamily,
    debug_comments: bool,
    default_sdk_symbols: bool,
) -> AssemblyOptions {
    let layout = layout_for_target(target, cpu);
    assembly_options_for_layout(&layout, cpu, target, debug_comments, default_sdk_symbols)
}

pub fn assembly_options_for_layout(
    layout: &Layout,
    cpu: CpuFamily,
    target: &str,
    debug_comments: bool,
    default_sdk_symbols: bool,
) -> AssemblyOptions {
    let symbol = |name: &str| {
        layout
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .map(|symbol| symbol.value)
    };
    let defaults = AssemblyOptions::default();
    let is_16_bit = memory_model_for_cpu(cpu).is_some_and(|memory| memory.address_width_bits <= 16);
    AssemblyOptions {
        cpu,
        debug_comments,
        default_sdk_symbols,
        dos_executable: is_msdos_i8086_target(target),
        mos_executable: layout.name == "agon_light_mos",
        c64_executable: matches!(layout.name.as_str(), "commodore64_6502" | "commodore64_crt"),
        ti_os_executable: target.starts_with("ti83-z80")
            || target.starts_with("ti83plus-z80")
            || target.starts_with("ti84-z80")
            || target.starts_with("ti84plus-z80")
            || target.starts_with("ti84plusce-ez80")
            || target.starts_with("ti83premiumce-ez80"),
        arduboy_executable: target.starts_with("arduboy-"),
        gameboy_banking: None,
        optimization: defaults.optimization.clone(),
        load_addr: symbol("EZRA_LOAD_ADDR").unwrap_or(layout.load),
        entry_addr: symbol("EZRA_ENTRY_ADDR").unwrap_or(layout.entry),
        code_base: symbol("EZRA_CODE_BASE").unwrap_or(layout.entry),
        stack_top: symbol("EZRA_STACK_TOP").unwrap_or(layout.stack),
        ram_base: symbol("EZRA_RAM_BASE")
            .or(is_16_bit.then_some(Address24::new(0xA000)))
            .unwrap_or(defaults.ram_base),
        vram_base: symbol("EZRA_VRAM_BASE")
            .or(is_16_bit.then_some(Address24::new(0)))
            .unwrap_or(defaults.vram_base),
        audio_base: symbol("EZRA_AUDIO_BASE")
            .or(is_16_bit.then_some(Address24::new(0)))
            .unwrap_or(defaults.audio_base),
        asset_base: symbol("EZRA_ASSET_BASE")
            .or(is_16_bit.then_some(Address24::new(0xC000)))
            .unwrap_or(defaults.asset_base),
        rodata_base: symbol("EZRA_RODATA_BASE")
            .or(is_16_bit.then_some(Address24::new(0x8000)))
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

fn validate_generated_assembly(
    assembly: &str,
    cpu: CpuFamily,
    layout: &Layout,
) -> Result<(), Diagnostic> {
    let assembled = assemble_subset_with_symbols_at(cpu.into(), assembly, layout.entry.get())?;
    validate_text_section_fit(layout, assembled_text_len(&assembled))
}

fn assembled_text_len(assembled: &crate::vm::AssembledProgram) -> usize {
    assembled
        .section_ranges
        .iter()
        .find(|section| section.name == ".text")
        .map(|section| section.end.saturating_sub(section.start) as usize)
        .unwrap_or(assembled.bytes.len())
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

fn assembly_source_options(source_path: &str, layout: &Layout) -> AssemblerSourceOptions {
    AssemblerSourceOptions {
        source_path: Some(source_path.to_owned()),
        symbols: layout
            .symbols
            .iter()
            .map(|symbol| AssemblySymbol {
                name: symbol.name.clone(),
                addr: symbol.value.get(),
            })
            .collect(),
        ..AssemblerSourceOptions::default()
    }
}

fn flat_assembly_map_at(
    code_len: usize,
    symbols: &[AssemblySymbol],
    base_addr: u32,
) -> Result<String, Diagnostic> {
    let code_len = u32::try_from(code_len)
        .map_err(|_| Diagnostic::new("assembled program exceeds the 24-bit address space"))?;
    let end = base_addr
        .checked_add(code_len.saturating_sub(1))
        .ok_or_else(|| Diagnostic::new("assembled program exceeds the 24-bit address space"))?;
    let mut out = format!(
        "section      start      end        size\n{:<12} 0x{:06X} 0x{:06X} 0x{:06X}\n",
        ".text", base_addr, end, code_len
    );
    if !symbols.is_empty() {
        out.push_str("\nsymbol       address\n");
        for symbol in symbols {
            out.push_str(&format!("{:<12} 0x{:06X}\n", symbol.name, symbol.addr));
        }
    }
    Ok(out)
}

#[cfg(all(test, feature = "i8086"))]
mod tests;
