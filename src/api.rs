//! Public, in-process compiler API.
//!
//! This module compiles EZRA source supplied by another Rust program without
//! invoking the `ezrac` CLI or writing build artifacts. It produces target
//! assembly, linked images, maps, symbols, and packaged executables.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

pub use crate::workspace::{Workspace, WorkspaceFile};

use crate::{
    asm::{
        AssemblyItem, AssemblyOptions, AssemblyPreprocessOptions, AssemblyProgram,
        emit_ez80_assembly_with_options, emit_lr35902_assembly_with_options,
        emit_mos6502_assembly_with_options, preprocess_assembly_source,
    },
    ast::{Declaration, Program},
    cart::{build_cartridge_map, collect_gameboy_banked_embeds},
    compile::{
        CompileOptions, CompileReport, SdkResolver, check_source_with_sdk_and_overrides,
        parse_and_resolve_imports_with_sdk_and_overrides,
        parse_and_resolve_imports_with_sdk_and_workspace,
    },
    diagnostic::Diagnostic,
    layout::{Layout, default_layout_for_target},
    package::{PackageContext, PackageRequest, package_executable_with_context},
    parser::parse_program,
    target::{
        Address24, AssemblerCpu, CpuFamily, DEFAULT_TARGET_TRIPLE, OutputFormat, TargetProfile,
        resolve_target_profile,
    },
    tbir::diagnostics::validate_program,
    vm::{
        AssemblerSourceOptions, AssemblySymbol, assemble_program_with_options_at,
        assemble_subset_with_options_at, assemble_subset_with_symbols_at,
        measure_assembly_program_with_options,
    },
    workspace::normalize_virtual_path,
};

#[cfg(feature = "dcpu")]
use crate::asm::emit_dcpu_assembly_with_options;
#[cfg(feature = "i8086")]
use crate::asm::emit_i8086_assembly_with_options;
#[cfg(feature = "m68k")]
use crate::asm::emit_m68k_assembly_with_options;
#[cfg(feature = "m6800")]
use crate::asm::emit_m6800_assembly_with_options;
#[cfg(feature = "tms9900")]
use crate::asm::emit_tms9900_assembly_with_options;

/// Options for compiling in-memory EZRA source to target assembly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileRequest {
    /// Logical path used for diagnostics and relative import resolution.
    pub source_path: PathBuf,
    /// Target triple used for SDK selection, validation, and code generation.
    pub target: String,
    /// Additional project SDK roots. Built-in SDKs are selected from `target`.
    pub sdk_paths: Vec<PathBuf>,
    /// Include generator debug comments in the emitted assembly where supported.
    pub debug_comments: bool,
    /// Enable default target SDK symbols.
    pub default_sdk_symbols: bool,
}

impl CompileRequest {
    /// Create a request with target-appropriate built-in SDK imports enabled.
    pub fn new(source_path: impl Into<PathBuf>, target: impl Into<String>) -> Self {
        Self {
            source_path: source_path.into(),
            target: target.into(),
            sdk_paths: Vec::new(),
            debug_comments: false,
            default_sdk_symbols: true,
        }
    }

    /// Construct a request for EZRA's default target.
    pub fn with_default_target(source_path: impl Into<PathBuf>) -> Self {
        Self::new(source_path, DEFAULT_TARGET_TRIPLE)
    }

    fn sdk_resolver(&self) -> SdkResolver {
        SdkResolver {
            target: Some(self.target.clone()),
            sdk_roots: self.sdk_paths.clone(),
        }
    }
}

impl Workspace<'_> {
    fn text_overrides(&self) -> Result<HashMap<PathBuf, String>, Diagnostic> {
        let mut overrides = HashMap::new();
        for file in self.files {
            let virtual_path = normalize_virtual_path(file.path);
            if !virtual_path.ends_with(".ezra") {
                continue;
            }
            let text = core::str::from_utf8(file.contents).map_err(|_| {
                Diagnostic::new(format!("workspace source `{}` is not UTF-8", file.path))
            })?;
            overrides.insert(PathBuf::from(&virtual_path), text.to_owned());
            let host_path = virtual_path.replace('/', std::path::MAIN_SEPARATOR_STR);
            overrides.insert(PathBuf::from(host_path), text.to_owned());
        }
        Ok(overrides)
    }
}

/// Successful in-process compilation output.
#[derive(Clone, Debug, PartialEq)]
pub struct AssemblyCompilation {
    /// Semantic/import report for the root source unit.
    pub report: CompileReport,
    /// Root program with imports resolved and public imported aliases added.
    pub program: Program,
    /// Target assembly text. The caller owns any further assembly/packaging.
    pub assembly: String,
}

/// Resolved build configuration independent of CLI flags, project discovery, and host paths.
///
/// Applications may construct this directly to use custom layouts, output formats,
/// assembler modes, and filesystem-free package metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildRequest {
    pub target: TargetProfile,
    pub output_format: OutputFormat,
    pub assembler_cpu: AssemblerCpu,
    pub layout: Layout,
    pub executable_name: Option<String>,
    pub package_context: PackageContext,
}

impl BuildRequest {
    /// Resolve a target's default layout and output format into a reusable build request.
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

/// Determines whether assembly is generated EZRA output or standalone assembly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkInput {
    Generated,
    Assembly,
}

/// Filesystem-free linked and packaged artifacts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkedCompilation {
    pub machine_code: Vec<u8>,
    pub map: String,
    pub symbols: Vec<AssemblySymbol>,
    pub executable: Vec<u8>,
    pub output_format: OutputFormat,
    pub executable_extension: &'static str,
}

/// Complete filesystem-free build output.
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

/// Compile in-memory EZRA source to target assembly.
///
/// The source must define `fn main()` because this API emits executable source
/// assembly. For library and SDK diagnostics without an entry point, use
/// [`crate::compile::check_module_diagnostics_with_sdk_and_overrides`] or an
/// EZRA project with `[lsp] mode = "library"`.
pub fn compile_source_to_assembly(
    source: &str,
    request: &CompileRequest,
) -> Result<AssemblyCompilation, Diagnostic> {
    compile_source_to_assembly_with_overrides(source, request, &HashMap::new())
}

/// Compile an Ezra source file using a caller-owned virtual workspace.
///
/// Imports resolve from `workspace` before host filesystem SDK roots. This
/// makes compilation deterministic for embedders and is the std-mode precursor
/// to the alloc-only workspace API.
pub fn compile_workspace_to_assembly(
    workspace: &Workspace<'_>,
    root: &str,
    request: &CompileRequest,
) -> Result<AssemblyCompilation, Diagnostic> {
    let root = normalize_virtual_path(root);
    let source = workspace.file(&root).ok_or_else(|| {
        Diagnostic::new(format!("workspace does not contain root source `{root}`"))
    })?;
    let source = core::str::from_utf8(source)
        .map_err(|_| Diagnostic::new(format!("workspace source `{root}` is not UTF-8")))?;
    let mut request = request.clone();
    request.source_path = PathBuf::from(&root);
    let target = resolve_target_profile(Some(&request.target)).map_err(Diagnostic::new)?;
    let layout = layout_for_target(&request.target, target.triple.cpu);
    validate_layout_for_cpu(&layout, target.triple.cpu, &request.target)?;
    let sdk = request.sdk_resolver();
    let overrides = workspace.text_overrides()?;
    let root_program = parse_program(&request.source_path, source)?;
    let imports = root_program
        .declarations
        .iter()
        .filter(|declaration| matches!(declaration, Declaration::Import(_)))
        .count();
    let program = parse_and_resolve_imports_with_sdk_and_workspace(
        &request.source_path,
        source,
        &sdk,
        &overrides,
        workspace,
    )?;
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
    let assembly = emit_source_assembly(&program, options)?;
    validate_generated_assembly(&assembly, target.triple.cpu, &layout)?;
    Ok(AssemblyCompilation {
        report,
        program,
        assembly,
    })
}

/// Compile, assemble, and package a virtual workspace entirely in memory using
/// the target's default build configuration.
pub fn build_workspace(
    workspace: &Workspace<'_>,
    root: &str,
    request: &CompileRequest,
) -> Result<BuildCompilation, Diagnostic> {
    let mut build = BuildRequest::for_target(&request.target)?;
    build.executable_name = root
        .rsplit('/')
        .next()
        .and_then(|name| name.split('.').next())
        .map(str::to_owned);
    build_workspace_with_request(workspace, root, request, &build)
}

/// Compile, assemble, and package a virtual workspace using caller-supplied
/// target, layout, output, and package settings.
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
    let compilation = compile_workspace_to_assembly(workspace, root, request)?;
    let linked = link_generated_assembly(
        &request.source_path,
        &compilation.assembly,
        &compilation.program,
        build,
    )?;

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

/// Link generated source assembly and package it using a caller-supplied build request.
///
/// This is the shared source-build path used by the CLI and virtual-workspace API.
pub fn link_generated_assembly(
    source_path: &Path,
    assembly: &str,
    program: &Program,
    build: &BuildRequest,
) -> Result<LinkedCompilation, Diagnostic> {
    validate_layout_for_cpu(
        &build.layout,
        build.target.triple.cpu,
        &build.target.triple.value,
    )?;
    let (machine_code, map, symbols) = if build.target.triple.cpu == CpuFamily::M68k {
        let preprocessed = preprocess_assembly_source(
            &source_path.to_string_lossy(),
            assembly,
            AssemblyPreprocessOptions::for_compiled_features(
                &build.target.triple.value,
                build.assembler_cpu.as_str(),
            ),
        )?;
        let image = link_assembly_program_image(source_path, &preprocessed.program, build)?;
        (image.bytes, image.map, image.symbols)
    } else {
        let assembled = assemble_subset_with_options_at(
            build.assembler_cpu,
            assembly,
            build.layout.entry.get(),
            &assembly_source_options(source_path, &build.layout),
        )?;
        validate_assembled_section_fit(
            &build.layout,
            ".text",
            build.layout.entry.get(),
            assembled.bytes.len(),
        )?;
        let map = build_output_map(build, program, assembled.bytes.len(), &assembled.symbols)?;
        (assembled.bytes, map, assembled.symbols)
    };
    package_generated_linked(build, program, machine_code, map, symbols)
}

fn package_generated_linked(
    build: &BuildRequest,
    program: &Program,
    machine_code: Vec<u8>,
    map: String,
    symbols: Vec<AssemblySymbol>,
) -> Result<LinkedCompilation, Diagnostic> {
    let mut build = build.clone();
    let mut resident_code = machine_code;
    if let Some(options) = build.package_context.game_boy.as_mut() {
        let generated =
            game_boy_banked_code_payloads(&resident_code, &symbols, build.layout.entry.get())?;
        let banked_code_start = symbols
            .iter()
            .filter(|symbol| {
                symbol.name.starts_with("__ezra_bank_") && symbol.name.ends_with("_start")
            })
            .map(|symbol| symbol.addr)
            .min();
        if let Some(start) = banked_code_start {
            let offset = usize::try_from(start.saturating_sub(build.layout.entry.get()))
                .unwrap_or(resident_code.len());
            resident_code.truncate(offset.min(resident_code.len()));
        }
        options.generated_bank_payloads = generated
            .into_iter()
            .map(|(bank, bytes)| crate::package::GameBoyBankPayload { bank, bytes })
            .collect();
        if options.generated_bank_payloads.is_empty() {
            for embed in collect_gameboy_banked_embeds(program)? {
                let bank = usize::try_from(embed.bank).map_err(|_| {
                    Diagnostic::new(format!(
                        "Game Boy bank {} is outside host range",
                        embed.bank
                    ))
                })?;
                let payload = options
                    .generated_bank_payloads
                    .iter_mut()
                    .find(|payload| payload.bank == bank);
                if let Some(payload) = payload {
                    payload.bytes.extend_from_slice(&embed.bytes);
                } else {
                    options
                        .generated_bank_payloads
                        .push(crate::package::GameBoyBankPayload {
                            bank,
                            bytes: embed.bytes,
                        });
                }
            }
        }
    }
    package_linked(&build, resident_code, map, symbols)
}

fn game_boy_banked_code_payloads(
    code: &[u8],
    symbols: &[AssemblySymbol],
    base: u32,
) -> Result<BTreeMap<usize, Vec<u8>>, Diagnostic> {
    let mut starts = BTreeMap::new();
    let mut ends = BTreeMap::new();
    for symbol in symbols {
        let Some(rest) = symbol.name.strip_prefix("__ezra_bank_") else {
            continue;
        };
        let Some((bank, suffix)) = rest.split_once('_') else {
            continue;
        };
        let bank = bank.parse::<usize>().map_err(|_| {
            Diagnostic::new(format!(
                "invalid generated Game Boy bank marker `{}`",
                symbol.name
            ))
        })?;
        match suffix {
            "start" => {
                starts.insert(bank, symbol.addr);
            }
            "end" => {
                ends.insert(bank, symbol.addr);
            }
            _ => {}
        }
    }
    let mut payloads = BTreeMap::new();
    for (bank, start) in starts {
        let end = ends.remove(&bank).ok_or_else(|| {
            Diagnostic::new(format!("generated Game Boy bank {bank} has no end marker"))
        })?;
        let start = usize::try_from(start.checked_sub(base).ok_or_else(|| {
            Diagnostic::new(format!(
                "generated Game Boy bank {bank} precedes resident code"
            ))
        })?)
        .map_err(|_| Diagnostic::new("generated Game Boy bank offset exceeds host range"))?;
        let end = usize::try_from(end.checked_sub(base).ok_or_else(|| {
            Diagnostic::new(format!(
                "generated Game Boy bank {bank} precedes resident code"
            ))
        })?)
        .map_err(|_| Diagnostic::new("generated Game Boy bank offset exceeds host range"))?;
        if start > end || end > code.len() {
            return Err(Diagnostic::new(format!(
                "generated Game Boy bank {bank} is outside assembled code"
            )));
        }
        payloads.insert(bank, code[start..end].to_vec());
    }
    if !ends.is_empty() {
        return Err(Diagnostic::new(
            "generated Game Boy bank end marker has no start marker",
        ));
    }
    Ok(payloads)
}

/// Link a preprocessed standalone assembly program and package it using a
/// caller-supplied build request. The caller owns filesystem include discovery.
pub fn link_assembly_program(
    source_path: &Path,
    program: &AssemblyProgram,
    build: &BuildRequest,
) -> Result<LinkedCompilation, Diagnostic> {
    validate_layout_for_cpu(
        &build.layout,
        build.target.triple.cpu,
        &build.target.triple.value,
    )?;
    let image = link_assembly_program_image(source_path, program, build)?;
    package_linked(build, image.bytes, image.map, image.symbols)
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct LinkedImage {
    bytes: Vec<u8>,
    map: String,
    symbols: Vec<AssemblySymbol>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AssemblySectionSource {
    name: String,
    program: AssemblyProgram,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlacedAssemblySection {
    name: String,
    start: u32,
    bytes: Vec<u8>,
}

fn link_assembly_program_image(
    source_path: &Path,
    program: &AssemblyProgram,
    build: &BuildRequest,
) -> Result<LinkedImage, Diagnostic> {
    let sections = split_assembly_sections(program);
    let section_bases = placed_assembly_section_bases(source_path, build, &sections)?;
    let mut options = assembly_source_options(source_path, &build.layout);
    options.section_bases = section_bases
        .iter()
        .map(|(name, start, _)| AssemblySymbol {
            name: name.clone(),
            addr: *start,
        })
        .collect();
    let assembled = assemble_program_with_options_at(
        build.assembler_cpu,
        program,
        build.layout.load.get(),
        &options,
    )?;
    let mut placed = Vec::new();
    for (name, start, len) in section_bases {
        validate_assembled_section_fit(&build.layout, &name, start, len)?;
        let offset = usize::try_from(start.saturating_sub(build.layout.load.get()))
            .map_err(|_| Diagnostic::new("assembly image exceeds host addressable memory"))?;
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Diagnostic::new("assembly image exceeds host addressable memory"))?;
        if end > assembled.bytes.len() {
            return Err(Diagnostic::new(format!(
                "assembled section `{name}` extends beyond the linked image"
            )));
        }
        placed.push(PlacedAssemblySection {
            name,
            start,
            bytes: assembled.bytes[offset..end].to_vec(),
        });
    }
    Ok(LinkedImage {
        bytes: assembly_image_bytes(build, &placed)?,
        map: assembly_section_map(&placed, &assembled.symbols),
        symbols: assembled.symbols,
    })
}

fn placed_assembly_section_bases(
    source_path: &Path,
    build: &BuildRequest,
    sections: &[AssemblySectionSource],
) -> Result<Vec<(String, u32, usize)>, Diagnostic> {
    let mut lengths = BTreeMap::new();
    for section in sections {
        let len = measure_assembly_program_with_options(
            build.assembler_cpu,
            &section.program,
            &AssemblerSourceOptions {
                source_path: Some(source_path.to_path_buf()),
                ..AssemblerSourceOptions::default()
            },
        )?;
        lengths.insert(section.name.clone(), len);
    }
    for name in lengths.keys() {
        if !build
            .layout
            .sections
            .iter()
            .any(|section| &section.name == name)
        {
            return Err(Diagnostic::new(format!(
                "assembly section `{name}` is not defined by layout `{}`",
                build.layout.name
            )));
        }
    }
    let mut cursors = BTreeMap::<String, u32>::new();
    let mut placed = Vec::new();
    for section in &build.layout.sections {
        let Some(len) = lengths.get(&section.name).copied() else {
            continue;
        };
        let region = build
            .layout
            .regions
            .iter()
            .find(|region| region.name == section.region)
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "layout section `{}` targets unknown region `{}`",
                    section.name, section.region
                ))
            })?;
        let cursor = cursors
            .entry(region.name.clone())
            .or_insert(region.start.get());
        let start = if section.name == ".text" {
            build.layout.entry.get()
        } else {
            align_u32(*cursor, section.align)?
        };
        let len_u32 = u32::try_from(len).map_err(|_| {
            Diagnostic::new(format!(
                "section `{}` exceeds 24-bit address space",
                section.name
            ))
        })?;
        *cursor = start.checked_add(len_u32).ok_or_else(|| {
            Diagnostic::new(format!(
                "section `{}` exceeds 24-bit address space",
                section.name
            ))
        })?;
        placed.push((section.name.clone(), start, len));
    }
    Ok(placed)
}

fn split_assembly_sections(program: &AssemblyProgram) -> Vec<AssemblySectionSource> {
    let mut sections = BTreeMap::<String, AssemblyProgram>::new();
    let mut current = ".text".to_owned();
    sections.insert(current.clone(), AssemblyProgram { items: Vec::new() });
    for item in &program.items {
        if let AssemblyItem::Section(name) = &item.kind {
            current = name.clone();
            sections
                .entry(current.clone())
                .or_insert_with(|| AssemblyProgram { items: Vec::new() });
        } else {
            sections
                .entry(current.clone())
                .or_insert_with(|| AssemblyProgram { items: Vec::new() })
                .items
                .push(item.clone());
        }
    }
    sections
        .into_iter()
        .map(|(name, program)| AssemblySectionSource { name, program })
        .collect()
}

fn validate_assembled_section_fit(
    layout: &Layout,
    name: &str,
    start: u32,
    len: usize,
) -> Result<(), Diagnostic> {
    if len == 0 {
        return Ok(());
    }
    let section = layout
        .sections
        .iter()
        .find(|section| section.name == name)
        .ok_or_else(|| {
            Diagnostic::new(format!(
                "assembly section `{name}` is not defined by layout `{}`",
                layout.name
            ))
        })?;
    let region = layout
        .regions
        .iter()
        .find(|region| region.name == section.region)
        .ok_or_else(|| {
            Diagnostic::new(format!(
                "layout section `{name}` targets unknown region `{}`",
                section.region
            ))
        })?;
    let end = start
        .checked_add(
            u32::try_from(len).map_err(|_| {
                Diagnostic::new(format!("section `{name}` exceeds 24-bit address space"))
            })? - 1,
        )
        .ok_or_else(|| Diagnostic::new(format!("section `{name}` exceeds 24-bit address space")))?;
    if start < region.start.get() || end > region.end.get() {
        return Err(Diagnostic::new(format!(
            "assembly section `{name}` range 0x{start:06X}..0x{end:06X} does not fit in region `{}`",
            region.name
        )));
    }
    Ok(())
}

fn assembly_image_bytes(
    build: &BuildRequest,
    sections: &[PlacedAssemblySection],
) -> Result<Vec<u8>, Diagnostic> {
    if build.output_format == OutputFormat::CpmCom {
        return Ok(sections
            .iter()
            .find(|section| section.name == ".text")
            .map(|section| section.bytes.clone())
            .unwrap_or_default());
    }
    let max_end = sections
        .iter()
        .filter(|section| !section.bytes.is_empty())
        .map(|section| section.start + section.bytes.len() as u32)
        .max()
        .unwrap_or(build.layout.load.get());
    let len = usize::try_from(max_end.saturating_sub(build.layout.load.get()))
        .map_err(|_| Diagnostic::new("assembly image exceeds host addressable memory"))?;
    let mut image = vec![0; len];
    for section in sections {
        let offset = section
            .start
            .checked_sub(build.layout.load.get())
            .ok_or_else(|| {
                Diagnostic::new(format!(
                    "section `{}` starts before layout load address",
                    section.name
                ))
            })?;
        let offset = usize::try_from(offset)
            .map_err(|_| Diagnostic::new("assembly image exceeds host addressable memory"))?;
        image[offset..offset + section.bytes.len()].copy_from_slice(&section.bytes);
    }
    Ok(image)
}

fn assembly_section_map(sections: &[PlacedAssemblySection], symbols: &[AssemblySymbol]) -> String {
    let mut out = String::from("section      start      end        size\n");
    for section in sections {
        let len = section.bytes.len() as u32;
        let end = section.start + len.saturating_sub(1);
        out.push_str(&format!(
            "{:<12} 0x{:06X} 0x{:06X} 0x{:06X}\n",
            section.name, section.start, end, len
        ));
    }
    if !symbols.is_empty() {
        out.push_str("\nsymbol       address\n");
        for symbol in symbols {
            out.push_str(&format!("{:<12} 0x{:06X}\n", symbol.name, symbol.addr));
        }
    }
    out
}

fn build_output_map(
    build: &BuildRequest,
    program: &Program,
    code_len: usize,
    symbols: &[AssemblySymbol],
) -> Result<String, Diagnostic> {
    if uses_flat_output_map(build) {
        let code_len = u32::try_from(code_len)
            .map_err(|_| Diagnostic::new("program code exceeds 24-bit address space"))?;
        let end = build
            .layout
            .entry
            .get()
            .checked_add(code_len.saturating_sub(1))
            .ok_or_else(|| Diagnostic::new("program code exceeds 24-bit address space"))?;
        return Ok(format!(
            "section      start      end        size\n{:<12} {} 0x{:06X} 0x{:06X}\n",
            ".text", build.layout.entry, end, code_len
        ));
    }
    build_cartridge_map(program, &build.layout, code_len, symbols)
}

fn uses_flat_output_map(build: &BuildRequest) -> bool {
    build.output_format == OutputFormat::CpmCom
        || bare_target_cpu(&build.target.triple.value).is_some()
        || build.target.triple.value.starts_with("zxspectrum-z80")
        || build.target.triple.value.starts_with("gameboy-")
        || build.target.triple.value.starts_with("arduboy-")
        || build.target.triple.value.starts_with("commodore64-6502")
        || build.target.triple.value.starts_with("ti84plusce-ez80")
        || build.target.triple.value.starts_with("ti83premiumce-ez80")
        || build.target.triple.value.starts_with("ti83-z80")
        || build.target.triple.value.starts_with("ti83plus-z80")
        || build.target.triple.value.starts_with("ti84-z80")
        || build.target.triple.value.starts_with("ti84plus-z80")
        || build.target.triple.value.starts_with("ti99-4a-tms9900")
}

fn bare_target_cpu(target: &str) -> Option<AssemblerCpu> {
    let parts = target.split('-').collect::<Vec<_>>();
    if !parts.contains(&"bare") {
        return None;
    }
    parts
        .into_iter()
        .find_map(|part| AssemblerCpu::parse(part).ok())
}

fn align_u32(value: u32, align: u32) -> Result<u32, Diagnostic> {
    if align <= 1 {
        return Ok(value);
    }
    let mask = align - 1;
    value
        .checked_add(mask)
        .map(|value| value & !mask)
        .ok_or_else(|| Diagnostic::new("aligned address exceeds 24-bit address space"))
}

fn assembly_source_options(source_path: &Path, layout: &Layout) -> AssemblerSourceOptions {
    AssemblerSourceOptions {
        source_path: Some(source_path.to_path_buf()),
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

fn compile_source_to_assembly_with_overrides(
    source: &str,
    request: &CompileRequest,
    source_overrides: &HashMap<PathBuf, String>,
) -> Result<AssemblyCompilation, Diagnostic> {
    let target = resolve_target_profile(Some(&request.target)).map_err(Diagnostic::new)?;
    let layout = layout_for_target(&request.target, target.triple.cpu);
    validate_layout_for_cpu(&layout, target.triple.cpu, &request.target)?;
    let sdk = request.sdk_resolver();
    let options = CompileOptions {
        source: request.source_path.clone(),
        debug_comments: request.debug_comments,
        default_sdk_symbols: request.default_sdk_symbols,
    };
    let report = check_source_with_sdk_and_overrides(source, &options, &sdk, source_overrides)?;
    let program = parse_and_resolve_imports_with_sdk_and_overrides(
        &request.source_path,
        source,
        &sdk,
        source_overrides,
    )?;
    let assembly_options = assembly_options_for_target(
        &request.target,
        target.triple.cpu,
        request.debug_comments,
        request.default_sdk_symbols,
    );
    let assembly = emit_source_assembly(&program, assembly_options)?;
    validate_generated_assembly(&assembly, target.triple.cpu, &layout)?;

    Ok(AssemblyCompilation {
        report,
        program,
        assembly,
    })
}

/// Build target assembly options from a target triple and its default layout.
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
    let is_16_bit = crate::target::memory_model_for_cpu(cpu)
        .is_some_and(|memory| memory.address_width_bits <= 16);

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
        gameboy_banking: None,
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
    let address_width_bits = crate::target::memory_model_for_cpu(cpu)
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

fn emit_source_assembly(program: &Program, options: AssemblyOptions) -> Result<String, Diagnostic> {
    validate_program(program, options.cpu)?;
    match options.cpu {
        CpuFamily::I8086 => {
            #[cfg(feature = "i8086")]
            {
                emit_i8086_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "i8086"))]
            {
                Err(Diagnostic::new(
                    "i8086 source compilation requires the `i8086` Cargo feature",
                ))
            }
        }
        CpuFamily::Lr35902 => emit_lr35902_assembly_with_options(program, options),
        CpuFamily::Mos6502 | CpuFamily::Cmos65C02 | CpuFamily::Wdc65C816 | CpuFamily::Ricoh2A03 => {
            emit_mos6502_assembly_with_options(program, options)
        }
        CpuFamily::Dcpu => {
            #[cfg(feature = "dcpu")]
            {
                emit_dcpu_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "dcpu"))]
            {
                Err(Diagnostic::new(
                    "DCPU-16 source compilation requires the `dcpu` Cargo feature",
                ))
            }
        }
        CpuFamily::M6800 => {
            #[cfg(feature = "m6800")]
            {
                emit_m6800_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "m6800"))]
            {
                Err(Diagnostic::new(
                    "M6800 source compilation requires the `m6800` Cargo feature",
                ))
            }
        }
        CpuFamily::Tms9900 => {
            #[cfg(feature = "tms9900")]
            {
                emit_tms9900_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "tms9900"))]
            {
                Err(Diagnostic::new(
                    "TMS9900 source compilation requires the `tms9900` Cargo feature",
                ))
            }
        }
        CpuFamily::M68k => {
            #[cfg(feature = "m68k")]
            {
                emit_m68k_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "m68k"))]
            {
                Err(Diagnostic::new(
                    "M68k source compilation requires the `m68k` Cargo feature",
                ))
            }
        }
        CpuFamily::Avr => Err(Diagnostic::new(format!(
            "EZRA source code generation is not implemented for CPU `{}`",
            options.cpu.as_str()
        ))),
        _ => emit_ez80_assembly_with_options(program, options),
    }
}

/// Resolve the source path used by a compilation request relative to a host
/// application without requiring it to exist on disk.
pub fn source_path(root: impl AsRef<Path>, relative: impl AsRef<Path>) -> PathBuf {
    root.as_ref().join(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{EmbedSource, Expr};

    fn materialized_embed_bytes(program: &Program, name: &str) -> Vec<u8> {
        let embed = program
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Embed(embed) if embed.name == name => Some(embed),
                _ => None,
            })
            .expect("materialized embed declaration");
        let EmbedSource::Bytes(values) = &embed.source else {
            panic!("workspace file embed was not materialized");
        };
        values
            .iter()
            .map(|value| match value {
                Expr::Int(value) => *value as u8,
                _ => panic!("materialized workspace byte is not an integer"),
            })
            .collect()
    }

    #[test]
    fn compiles_in_memory_source_to_ez80_assembly() {
        let request = CompileRequest::new("memory.ezra", "custom-unknown-ez80");
        let compilation = compile_source_to_assembly("fn main() {}", &request).unwrap();

        assert!(compilation.report.has_main);
        assert!(compilation.assembly.contains("__ezra_start:"));
    }

    #[cfg(feature = "i8086")]
    #[test]
    fn compiles_in_memory_source_to_i8086_assembly() {
        let request = CompileRequest::new("memory.ezra", "bare-i8086");
        let compilation = compile_source_to_assembly(
            "fn twice(value: u16) -> u16 { return value * 2 }\nfn main() { let value: u16 = twice(21) }",
            &request,
        )
        .unwrap();

        assert!(compilation.report.has_main);
        assert!(compilation.assembly.contains("target: Intel 8086"));
        assert!(compilation.assembly.contains("call near _twice"));
    }

    #[cfg(feature = "i8086")]
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

    #[cfg(feature = "i8086")]
    #[test]
    fn imported_aliases_are_resolved_before_i8086_inline_asm_class_validation() {
        let files = [
            WorkspaceFile::text(
                "main.ezra",
                "import types\nfn main() { let value: Word = 1 asm(in value: Word as reg16, clobber ax) { \"nop\" } }",
            ),
            WorkspaceFile::text("types.ezra", "pub alias Word = u16"),
        ];
        let compilation = compile_workspace_to_assembly(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", "bare-i8086"),
        )
        .unwrap();

        assert!(compilation.assembly.contains("mov ax,"));
    }

    #[cfg(feature = "i8086")]
    #[test]
    fn virtual_workspace_compilation_strictly_validates_i8086_inline_assembly() {
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
    fn compiles_imports_from_a_virtual_workspace() {
        let files = [
            WorkspaceFile::text(
                "src/main.ezra",
                "import math\nfn main() { let value: u8 = math.VALUE }\n",
            ),
            WorkspaceFile::text("src/math.ezra", "pub const VALUE: u8 = 42\n"),
        ];
        let request = CompileRequest::new("ignored.ezra", "custom-unknown-ez80");
        let compilation =
            compile_workspace_to_assembly(&Workspace::new(&files), "src/main.ezra", &request)
                .unwrap();

        assert!(compilation.report.has_main);
        assert!(compilation.assembly.contains("_main:"));
    }

    #[test]
    fn materializes_root_relative_workspace_assets() {
        let files = [
            WorkspaceFile::text(
                "src/main.ezra",
                "embed blob: bytes = file(\"assets/blob.bin\")\nfn main() {}\n",
            ),
            WorkspaceFile::new("src/assets/blob.bin", &[0xA5, 0x00, 0xFF]),
        ];
        let compilation = compile_workspace_to_assembly(
            &Workspace::new(&files),
            "src/main.ezra",
            &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
        )
        .unwrap();

        assert_eq!(
            materialized_embed_bytes(&compilation.program, "blob"),
            [0xA5, 0x00, 0xFF]
        );
    }

    #[test]
    fn materializes_imported_module_relative_workspace_assets() {
        let files = [
            WorkspaceFile::text("src/main.ezra", "import lib.media\nfn main() {}\n"),
            WorkspaceFile::text(
                "src/lib/media.ezra",
                "pub embed sprite: bytes = file(\"assets/sprite.bin\")\n",
            ),
            WorkspaceFile::new("src/lib/assets/sprite.bin", &[0xDE, 0xAD]),
        ];
        let build = build_workspace(
            &Workspace::new(&files),
            "src/main.ezra",
            &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
        )
        .unwrap();

        assert_eq!(
            materialized_embed_bytes(&build.program, "sprite"),
            [0xDE, 0xAD]
        );
        assert!(!build.machine_code.is_empty());
    }

    #[test]
    fn reports_missing_virtual_workspace_assets() {
        let files = [WorkspaceFile::text(
            "src/main.ezra",
            "embed blob: bytes = file(\"assets/missing.bin\")\nfn main() {}\n",
        )];
        let error = compile_workspace_to_assembly(
            &Workspace::new(&files),
            "src/main.ezra",
            &CompileRequest::new("ignored.ezra", "cpm-2.2-z80"),
        )
        .unwrap_err();

        assert_eq!(
            error.message,
            "virtual workspace asset `assets/missing.bin` referenced from `src/main.ezra` was not found (resolved as `src/assets/missing.bin`)"
        );
    }

    #[test]
    fn builds_and_packages_virtual_workspace_for_agon() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let build = build_workspace(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", "agonlight-mos-ez80"),
        )
        .unwrap();

        assert_eq!(build.executable_extension, "bin");
        assert_eq!(&build.executable[64..69], b"MOS\0\x01");
        assert!(!build.machine_code.is_empty());
    }

    #[test]
    fn builds_and_packages_virtual_workspace_for_cpm() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let build = build_workspace(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", "cpm-2.2-z80"),
        )
        .unwrap();

        assert_eq!(build.executable_extension, "com");
        assert_eq!(build.executable, build.machine_code);
    }

    #[cfg(feature = "mos6502")]
    #[test]
    fn builds_and_packages_virtual_workspace_for_c64() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let build = build_workspace(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", "commodore64-6502"),
        )
        .unwrap();

        assert_eq!(build.executable_extension, "prg");
        assert_eq!(&build.executable[..2], &[0x01, 0x08]);
    }

    #[test]
    fn links_multisection_assembly_with_a_public_build_request() {
        let mut build = BuildRequest::for_target("agonlight-mos-ez80").unwrap();
        build.package_context.image_kind = crate::package::PackageImageKind::LoadImage;
        let assembly = r#"
            section .header
                db 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0A5h
            section .text
                ld hl, rodata_value
                ld de, data_value
                ret
            section .rodata
            rodata_value:
                db 0AAh, 0BBh
            section .data
            data_value:
                db 0CCh, 0DDh
        "#;
        let preprocessed = preprocess_assembly_source(
            "memory.asm",
            assembly,
            AssemblyPreprocessOptions::for_compiled_features("agonlight-mos-ez80", "ez80"),
        )
        .unwrap();
        let linked =
            link_assembly_program(Path::new("memory.asm"), &preprocessed.program, &build).unwrap();

        assert!(linked.map.contains(".rodata"));
        assert!(linked.map.contains(".data"));
        assert_eq!(&linked.executable[64..69], b"MOS\0\x01");
        assert_eq!(linked.executable[10], 0xA5);
    }

    #[test]
    fn workspace_build_honors_explicit_output_format() {
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let request = CompileRequest::new("main.ezra", "custom-unknown-ez80");
        let mut build = BuildRequest::for_target("custom-unknown-ez80").unwrap();
        build.output_format = OutputFormat::IntelHex;
        let result =
            build_workspace_with_request(&Workspace::new(&files), "main.ezra", &request, &build)
                .unwrap();

        assert_eq!(result.output_format, OutputFormat::IntelHex);
        assert_eq!(result.executable_extension, "hex");
        assert!(result.executable.starts_with(b":02000004"));
        assert!(result.executable.ends_with(b":00000001FF\n"));
    }

    #[test]
    fn rejects_incompatible_platform_cpu_combinations() {
        let request = CompileRequest::new("memory.ezra", "zxspectrum-ez80");
        let error = compile_source_to_assembly("fn main() {}", &request).unwrap_err();

        assert_eq!(
            error.message,
            "target `zxspectrum-ez80` requires CPU `z80`, not `ez80`"
        );
    }

    #[test]
    fn resolves_sdk_roots_for_in_memory_compilation() {
        let root = std::env::temp_dir().join(format!("ezrac-api-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("math.ezra"), "pub const VALUE: u8 = 42\n").unwrap();
        let mut request = CompileRequest::new(root.join("main.ezra"), "custom-unknown-ez80");
        request.sdk_paths.push(root.clone());

        let compilation = compile_source_to_assembly(
            "import math\nfn main() { let value: u8 = math.VALUE }\n",
            &request,
        )
        .unwrap();
        assert!(compilation.report.has_main);

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(feature = "i8086")]
    #[test]
    fn builds_msdos_com_as_raw_bytes_at_0100h_with_a_reserved_psp() {
        let profile = resolve_target_profile(Some("msdos-com-i8086")).unwrap();
        let layout = default_layout_for_target("msdos-com-i8086");
        let options = assembly_options_for_target("msdos-com-i8086", CpuFamily::I8086, false, true);
        let files = [WorkspaceFile::text("main.ezra", "fn main() {}")];
        let build = build_workspace(
            &Workspace::new(&files),
            "main.ezra",
            &CompileRequest::new("main.ezra", "msdos-com-i8086"),
        )
        .unwrap();
        let start = build
            .symbols
            .iter()
            .find(|symbol| symbol.name == "__ezra_start")
            .unwrap();
        let psp = layout
            .regions
            .iter()
            .find(|region| region.name == "psp")
            .unwrap();

        assert_eq!(profile.output_format, OutputFormat::CpmCom);
        assert_eq!(profile.output_format.extension(), "com");
        assert_eq!(layout.load.get(), 0x0100);
        assert_eq!(layout.entry.get(), 0x0100);
        assert_eq!((psp.start.get(), psp.end.get()), (0, 0x00FF));
        assert!(psp.flags.contains(crate::layout::RegionFlags::RESERVED));
        assert!(
            layout
                .regions
                .iter()
                .all(|region| region.end.get() <= 0xFFFF)
        );
        assert!(
            layout
                .symbols
                .iter()
                .all(|symbol| symbol.value.get() <= 0xFFFF)
        );
        assert_eq!(options.entry_addr.get(), 0x0100);
        assert!(options.dos_executable);
        assert_eq!(start.addr, 0x0100);
        assert_eq!(build.executable_extension, "com");
        assert_eq!(build.executable, build.machine_code);
        assert!(build.assembly.contains("    mov ax,0x4c00\n    int 0x21\n"));
    }

    #[test]
    fn rejects_non_i8086_and_noncanonical_msdos_targets() {
        let cpu_error = resolve_target_profile(Some("msdos-com-z80")).unwrap_err();
        assert_eq!(
            cpu_error,
            "target `msdos-com-z80` requires CPU `i8086`, not `z80`"
        );

        #[cfg(feature = "i8086")]
        {
            let name_error = resolve_target_profile(Some("msdos-i8086")).unwrap_err();
            assert_eq!(
                name_error,
                "unsupported MS-DOS target `msdos-i8086`; expected `msdos-com-i8086`"
            );
        }
    }
}
