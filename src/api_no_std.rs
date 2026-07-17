//! Alloc-only in-process compiler API backed exclusively by a virtual workspace.

use crate::compat::prelude::*;

pub use crate::workspace::{Workspace, WorkspaceFile};

#[cfg(feature = "i8086")]
use crate::asm::emit_i8086_assembly_with_options;
#[cfg(feature = "mos6502")]
use crate::asm::emit_mos6502_assembly_with_options;

use crate::{
    asm::{AssemblyOptions, emit_ez80_assembly_with_options},
    ast::{CfgPredicate, Declaration, Program},
    diagnostic::Diagnostic,
    layout::{Layout, default_layout_for_target},
    package::{PackageRequest, package_executable},
    parser::parse_program,
    target::{
        Address24, AssemblerCpu, CpuFamily, DEFAULT_TARGET_TRIPLE, OutputFormat,
        memory_model_for_cpu, resolve_target_profile,
    },
    vm::{AssemblySymbol, assemble_subset_with_symbols_at},
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
}

impl CompileRequest {
    pub fn new(source_path: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            source_path: source_path.into(),
            target: target.into(),
            sdk_paths: Vec::new(),
            debug_comments: false,
            default_sdk_symbols: true,
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

#[derive(Clone, Debug, PartialEq)]
pub struct BuildCompilation {
    pub report: CompileReport,
    pub program: Program,
    pub assembly: String,
    pub machine_code: Vec<u8>,
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
    if !matches!(
        target.triple.cpu,
        CpuFamily::Ez80
            | CpuFamily::Z80
            | CpuFamily::Z80N
            | CpuFamily::Z180
            | CpuFamily::I8080
            | CpuFamily::I8085
            | CpuFamily::I8086
            | CpuFamily::Mos6502
            | CpuFamily::Cmos65C02
            | CpuFamily::Wdc65C816
            | CpuFamily::Ricoh2A03
    ) {
        return Err(Diagnostic::new(format!(
            "no-std source code generation is currently available only for eZ80/Z80, i8086, and MOS 6502-family targets, not `{}`",
            target.triple.cpu.as_str()
        )));
    }

    let root = normalize_virtual_path(root);
    let source = workspace_text(workspace, &root)?;
    let mut root_program = parse_program(&root, source)?;
    materialize_workspace_embeds(&mut root_program, workspace)?;
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
    if main.return_type.is_some() {
        return Err(Diagnostic::new("main function cannot return a value"));
    }

    let report = CompileReport {
        imports,
        declarations: program.declarations.len(),
        has_main,
    };
    let options = assembly_options_for_target(
        &request.target,
        target.triple.cpu,
        request.debug_comments,
        request.default_sdk_symbols,
    );
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
        _ => emit_ez80_assembly_with_options(&program, options)?,
    };
    validate_generated_assembly(&assembly, target.triple.cpu, &layout)?;
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
    let target = resolve_target_profile(Some(&request.target)).map_err(Diagnostic::new)?;
    let layout = layout_for_target(&request.target, target.triple.cpu);
    validate_layout_for_cpu(&layout, target.triple.cpu, &request.target)?;
    let compilation = compile_workspace_to_assembly(workspace, root, request)?;
    let assembled = assemble_subset_with_symbols_at(
        AssemblerCpu::from(target.triple.cpu),
        &compilation.assembly,
        layout.entry.get(),
    )?;
    validate_text_section_fit(&layout, assembled.bytes.len())?;
    let root = normalize_virtual_path(root);
    let package_request = PackageRequest {
        target: request.target.clone(),
        output_format: target.output_format,
        load_addr: layout.load.get(),
        entry_addr: layout.entry.get(),
        executable_name: root
            .rsplit('/')
            .next()
            .and_then(|name| name.split('.').next())
            .map(str::to_owned),
    };
    let executable = package_executable(&package_request, &assembled.bytes)
        .map_err(|error| Diagnostic::new(error.message))?;

    Ok(BuildCompilation {
        report: compilation.report,
        program: compilation.program,
        assembly: compilation.assembly,
        machine_code: assembled.bytes,
        symbols: assembled.symbols,
        executable,
        output_format: target.output_format,
        executable_extension: if request.target.starts_with("gameboy-color-") {
            "gbc"
        } else {
            target.output_format.extension()
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
        materialize_workspace_embeds(&mut imported, workspace)?;
        imported.declarations = active_declarations(imported.declarations, request)?;
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
    validate_text_section_fit(layout, assembled.bytes.len())
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

#[cfg(all(test, feature = "i8086"))]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_i8086_target_uses_a_16_bit_layout() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let request = CompileRequest::new("main.ezra", "custom-board-i8086");
        let build = build_workspace(&Workspace::new(&files), "main.ezra", &request).unwrap();
        let options = assembly_options_for_target(&request.target, CpuFamily::I8086, false, true);

        assert_eq!(options.entry_addr.get(), 0);
        assert_eq!(options.stack_top.get(), 0xFFFF);
        assert!(!build.machine_code.is_empty());
    }

    #[test]
    fn compilation_strictly_validates_i8086_inline_assembly() {
        let files = [WorkspaceFile::text(
            "main.ezra",
            "fn main() { asm volatile { \"pusha\" } }",
        )];
        let error = compile_workspace_to_assembly(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", "bare-i8086"),
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
    fn build_layout_validation_rejects_text_that_exceeds_its_region() {
        let layout = Layout::bare_16("i8086");
        let error = validate_text_section_fit(&layout, 0x8001).unwrap_err();

        assert_eq!(
            error.message,
            "section `.text` does not fit in region `code`"
        );
    }

    #[test]
    fn alloc_only_api_builds_raw_msdos_com_images() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let request = CompileRequest::new("main.ezra", "msdos-com-i8086");
        let build = build_workspace(&Workspace::new(&files), "main.ezra", &request).unwrap();
        let start = build
            .symbols
            .iter()
            .find(|symbol| symbol.name == "__ezra_start")
            .unwrap();

        assert_eq!(start.addr, 0x0100);
        assert_eq!(build.output_format, OutputFormat::CpmCom);
        assert_eq!(build.executable_extension, "com");
        assert_eq!(build.executable, build.machine_code);
        assert!(build.assembly.contains("    int 0x21\n"));
    }
}
