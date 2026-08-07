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
        GameBoyBankingOptions, emit_ez80_assembly_with_options, emit_lr35902_assembly_with_options,
        emit_mos6502_assembly_with_options, preprocess_assembly_source,
    },
    ast::{Declaration, Program},
    cart::{
        build_cartridge_map, collect_gameboy_banked_embeds, layout_section_bases,
        source_section_sizes,
    },
    compile::{
        CompileOptions, CompileReport, SdkResolver, check_source_with_sdk_and_overrides,
        parse_and_resolve_imports_with_sdk_and_overrides,
        parse_and_resolve_imports_with_sdk_and_workspace,
    },
    diagnostic::{Diagnostic, SourceLocation},
    layout::{Layout, default_layout_for_target},
    package::{PackageContext, PackageRequest, package_executable_with_context},
    parser::parse_program,
    target::{
        Address24, AssemblerCpu, CpuFamily, DEFAULT_TARGET_TRIPLE, OutputFormat, TargetProfile,
        is_msdos_i8086_target, resolve_target_profile,
    },
    tbir::diagnostics::validate_program,
    vm::{
        AssemblerSourceOptions, AssemblySymbol, assemble_program_with_options_at,
        assemble_subset_with_options_at, assemble_subset_with_symbols_at,
        measure_assembly_program_with_options,
    },
    workspace::normalize_virtual_path,
};

#[cfg(feature = "avr")]
use crate::asm::emit_avr_assembly_with_options;
#[cfg(feature = "dcpu")]
use crate::asm::emit_dcpu_assembly_with_options;
#[cfg(feature = "i8086")]
use crate::asm::emit_i8086_assembly_with_options;
#[cfg(feature = "m68k")]
use crate::asm::emit_m68k_assembly_with_options;
#[cfg(feature = "m6800")]
use crate::asm::emit_m6800_assembly_with_options;
#[cfg(feature = "m6809")]
use crate::asm::emit_m6809_assembly_with_options;
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
    /// Optimization level and per-pass overrides.
    pub optimization: crate::optimization::OptimizationOptions,
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
            optimization: crate::optimization::OptimizationOptions::default(),
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

/// A named byte budget applied after linking and packaging.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SizeBudgets {
    /// Maximum final package size. This is the target artifact size budget.
    pub target: Option<usize>,
    /// Maximum sizes keyed by section name, or by one of the report fields such as
    /// `machine_code_payload`, `address_span`, or `final_package`.
    pub sections: BTreeMap<String, usize>,
    /// Maximum runtime-helper size when helper symbols make that size derivable.
    pub runtime_helpers: Option<usize>,
}

impl SizeBudgets {
    pub fn section(mut self, name: impl Into<String>, limit: usize) -> Self {
        self.sections.insert(name.into(), limit);
        self
    }

    pub fn with_target(mut self, limit: usize) -> Self {
        self.target = Some(limit);
        self
    }

    pub fn with_runtime_helpers(mut self, limit: usize) -> Self {
        self.runtime_helpers = Some(limit);
        self
    }
}

/// Size of one emitted address section. `size` excludes address gaps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSectionSize {
    pub name: String,
    pub size: usize,
}

/// Deterministic size breakdown for a linked and packaged artifact.
///
/// `machine_code_payload` is the sum of emitted section bytes excluding `.bss`.
/// `address_span` includes the address gaps between placed sections. `final_package`
/// is the number of bytes in the selected output format, including format headers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSizeReport {
    pub text: usize,
    pub rodata: usize,
    pub initialized_data: usize,
    pub assets: usize,
    pub bss: usize,
    pub runtime_helpers: Option<usize>,
    pub machine_code_payload: usize,
    pub address_span: usize,
    pub address_gaps: usize,
    pub final_package: usize,
    pub sections: Vec<ArtifactSectionSize>,
}

impl ArtifactSizeReport {
    #[cfg(test)]
    fn from_sections(
        sections: &[ArtifactSectionMeasurement],
        symbols: &[AssemblySymbol],
        final_package: usize,
    ) -> Self {
        Self::from_sections_with_storage(sections, symbols, final_package, None)
    }

    fn from_sections_with_storage(
        sections: &[ArtifactSectionMeasurement],
        symbols: &[AssemblySymbol],
        final_package: usize,
        source_storage: Option<&BTreeMap<String, usize>>,
    ) -> Self {
        let mut named = BTreeMap::<String, usize>::new();
        let mut min_start = None;
        let mut max_end = None;
        let mut physical_payload = 0usize;
        let mut physical_bytes = 0usize;

        for section in sections {
            *named.entry(section.name.clone()).or_default() += section.size;
            physical_bytes = physical_bytes.saturating_add(section.size);
            if section.name != ".bss" {
                physical_payload = physical_payload.saturating_add(section.size);
            }
            if section.size > 0 {
                min_start =
                    Some(min_start.map_or(section.start, |start: u32| start.min(section.start)));
                max_end = Some(max_end.map_or(section.end, |end: u32| end.max(section.end)));
            }
        }

        let address_span = match (min_start, max_end) {
            (Some(start), Some(end)) => usize::try_from(end.saturating_sub(start)).unwrap_or(0),
            _ => 0,
        };
        let address_gaps = address_span.saturating_sub(physical_bytes);

        // Source-level storage is separate from the linked byte payload. The
        // eZ80 emitter, for example, initializes globals and embeds from `.text`
        // but their address-backed `.data`/`.assets` sizes still belong in the
        // report. Keep physical payload accounting based only on linked sections.
        if let Some(storage) = source_storage {
            for (name, size) in storage {
                named.insert(name.clone(), *size);
            }
        }

        let text = named.get(".text").copied().unwrap_or(0);
        let rodata = named.get(".rodata").copied().unwrap_or(0);
        let initialized_data = named.get(".data").copied().unwrap_or(0);
        let bss = named.get(".bss").copied().unwrap_or(0);
        let assets = source_storage
            .and_then(|storage| storage.get(".assets").copied())
            .unwrap_or_else(|| {
                named
                    .iter()
                    .filter(|(name, _)| name.as_str() == ".assets" || name.starts_with(".assets:"))
                    .map(|(_, size)| *size)
                    .sum()
            });
        let runtime_helpers = runtime_helper_bytes(sections, symbols);
        let sections = named
            .into_iter()
            .map(|(name, size)| ArtifactSectionSize { name, size })
            .collect();

        Self {
            text,
            rodata,
            initialized_data,
            assets,
            bss,
            runtime_helpers,
            machine_code_payload: physical_payload,
            address_span,
            address_gaps,
            final_package,
            sections,
        }
    }

    /// Serialize the report in a versioned, deterministic line format suitable
    /// for checked-in baselines and build output files.
    pub fn to_stable_string(&self) -> String {
        let mut output = format!(
            "ezrac-size-v1\ntext={}\nrodata={}\ninitialized_data={}\nassets={}\nbss={}\nruntime_helpers={}\nmachine_code_payload={}\naddress_span={}\naddress_gaps={}\nfinal_package={}\n",
            self.text,
            self.rodata,
            self.initialized_data,
            self.assets,
            self.bss,
            self.runtime_helpers
                .map_or_else(|| "unknown".to_owned(), |size| size.to_string()),
            self.machine_code_payload,
            self.address_span,
            self.address_gaps,
            self.final_package,
        );
        for section in &self.sections {
            output.push_str(&format!("section:{}={}\n", section.name, section.size));
        }
        output
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ArtifactSectionMeasurement {
    name: String,
    start: u32,
    end: u32,
    size: usize,
}

fn runtime_helper_bytes(
    sections: &[ArtifactSectionMeasurement],
    symbols: &[AssemblySymbol],
) -> Option<usize> {
    let text_ranges = sections
        .iter()
        .filter(|section| section.name == ".text" && section.size > 0)
        .collect::<Vec<_>>();
    if text_ranges.is_empty() {
        return None;
    }

    let mut addresses = symbols.iter().map(|symbol| symbol.addr).collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    let helper_symbols = symbols
        .iter()
        .filter(|symbol| is_runtime_helper_symbol(&symbol.name))
        .collect::<Vec<_>>();
    if helper_symbols.is_empty() {
        return None;
    }

    let mut intervals = Vec::<(u32, u32)>::new();
    for symbol in helper_symbols {
        let Some(end) = addresses
            .iter()
            .copied()
            .find(|address| *address > symbol.addr)
        else {
            continue;
        };
        for text in &text_ranges {
            let start = symbol.addr.max(text.start);
            let end = end.min(text.end);
            if start < end {
                intervals.push((start, end));
            }
        }
    }
    if intervals.is_empty() {
        return None;
    }
    intervals.sort_unstable();
    let mut total = 0usize;
    let mut current = intervals[0];
    for interval in intervals.into_iter().skip(1) {
        if interval.0 <= current.1 {
            current.1 = current.1.max(interval.1);
        } else {
            total += usize::try_from(current.1.saturating_sub(current.0)).unwrap_or(0);
            current = interval;
        }
    }
    total += usize::try_from(current.1.saturating_sub(current.0)).unwrap_or(0);
    Some(total)
}

fn is_runtime_helper_symbol(name: &str) -> bool {
    [
        "__ezra_pass",
        "__ezra_fail",
        "__ezra_mem",
        "__ezra_mul_",
        "__ezra_div_",
        "__ezra_mod_",
        "__ezra_u16_",
        "__ezra_gb_",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

fn report_budget_value(report: &ArtifactSizeReport, name: &str) -> Option<usize> {
    match name {
        "text" | ".text" => Some(report.text),
        "rodata" | ".rodata" => Some(report.rodata),
        "data" | ".data" | "initialized_data" | "initialized-data" => Some(report.initialized_data),
        "assets" | ".assets" => Some(report.assets),
        "bss" | ".bss" => Some(report.bss),
        "runtime_helpers" | "runtime-helpers" | "helpers" => report.runtime_helpers,
        "machine_code_payload" | "machine-code-payload" => Some(report.machine_code_payload),
        "address_span" | "address-span" => Some(report.address_span),
        "address_gaps" | "address-gaps" => Some(report.address_gaps),
        "final_package" | "final-package" | "package" => Some(report.final_package),
        _ => report
            .sections
            .iter()
            .find(|section| section.name == name)
            .map(|section| section.size),
    }
}

fn validate_size_budgets(
    report: &ArtifactSizeReport,
    budgets: &SizeBudgets,
) -> Result<(), Diagnostic> {
    if let Some(limit) = budgets.target
        && report.final_package > limit
    {
        return Err(Diagnostic::new(format!(
            "target package size budget exceeded: final package is {} bytes, limit is {}; reduce packaged headers/payload or raise the target budget",
            report.final_package, limit
        )));
    }
    if let Some(limit) = budgets.runtime_helpers {
        match report.runtime_helpers {
            Some(actual) if actual > limit => {
                return Err(Diagnostic::new(format!(
                    "runtime-helper size budget exceeded: helpers use {} bytes, limit is {}; remove helper-using calls or raise the runtime-helper budget",
                    actual, limit
                )));
            }
            None => {
                return Err(Diagnostic::new(
                    "runtime-helper size budget cannot be checked: the linked artifact has no derivable helper symbol spans",
                ));
            }
            _ => {}
        }
    }
    for (name, limit) in &budgets.sections {
        let Some(actual) = report_budget_value(report, name) else {
            return Err(Diagnostic::new(format!(
                "size budget names unknown section or metric `{name}`; use `.text`, `.rodata`, `.data`, `.bss`, `.assets`, `runtime_helpers`, `machine_code_payload`, `address_span`, or `final_package`"
            )));
        };
        if actual > *limit {
            return Err(Diagnostic::new(format!(
                "size budget exceeded for `{name}`: {} bytes, limit is {}; reduce that section or raise its budget",
                actual, limit
            )));
        }
    }
    Ok(())
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
    pub gameboy_banking: Option<GameBoyBankingOptions>,
    pub package_context: PackageContext,
    pub size_budgets: SizeBudgets,
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
            gameboy_banking: None,
            package_context: PackageContext::new(),
            size_budgets: SizeBudgets::default(),
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
    pub size_report: ArtifactSizeReport,
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
    pub size_report: ArtifactSizeReport,
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
    let build = BuildRequest::for_target(&request.target)?;
    compile_workspace_to_assembly_with_request(workspace, root, request, &build)
}

/// Compile a virtual workspace using the layout and code-generation settings in
/// a resolved build request.
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
    let root = normalize_virtual_path(root);
    let source = workspace.file(&root).ok_or_else(|| {
        Diagnostic::new(format!("workspace does not contain root source `{root}`"))
    })?;
    let source = core::str::from_utf8(source)
        .map_err(|_| Diagnostic::new(format!("workspace source `{root}` is not UTF-8")))?;
    let mut request = request.clone();
    request.source_path = PathBuf::from(&root);
    validate_layout_for_cpu(
        &build.layout,
        build.target.triple.cpu,
        &build.target.triple.value,
    )?;
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
    if main.return_type.is_some() || main.second_return_type.is_some() {
        return Err(Diagnostic::new("main function cannot return a value"));
    }
    let report = CompileReport {
        imports,
        declarations: program.declarations.len(),
        has_main,
    };
    let options = assembly_options_for_layout_and_program(
        &build.layout,
        &program,
        build.target.triple.cpu,
        &build.target.triple.value,
        request.debug_comments,
        request.default_sdk_symbols,
        build.gameboy_banking,
    )?;
    let assembly = emit_source_assembly(&program, options)?;
    validate_generated_assembly(&assembly, build.target.triple.cpu, &build.layout)?;
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
    let compilation = compile_workspace_to_assembly_with_request(workspace, root, request, build)?;
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
        size_report: linked.size_report,
        executable: linked.executable,
        output_format: linked.output_format,
        executable_extension: linked.executable_extension,
    })
}

/// Strictly validate generated assembly against the target assembler and the
/// build request's `.text` region.
pub fn validate_generated_assembly_for_request(
    source_path: &Path,
    assembly: &str,
    build: &BuildRequest,
) -> Result<(), Diagnostic> {
    let assembled = assemble_subset_with_options_at(
        build.assembler_cpu,
        assembly,
        build.layout.entry.get(),
        &assembly_source_options(source_path, &build.layout),
    )?;
    let text = assembled
        .section_ranges
        .iter()
        .find(|section| section.name == ".text");
    validate_assembled_section_fit(
        &build.layout,
        ".text",
        text.map_or(build.layout.entry.get(), |section| section.start),
        text.map_or(assembled.bytes.len(), |section| {
            section.end.saturating_sub(section.start) as usize
        }),
    )
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
    let preprocessed = preprocess_assembly_source(
        &source_path.to_string_lossy(),
        assembly,
        AssemblyPreprocessOptions::for_compiled_features(
            &build.target.triple.value,
            build.assembler_cpu.as_str(),
        ),
    )?;
    let image = link_assembly_program_image(source_path, &preprocessed.program, build)?;
    let text_len = image
        .sections
        .iter()
        .find(|section| section.name == ".text")
        .map(|section| section.size)
        .unwrap_or(0);
    validate_assembled_section_fit(&build.layout, ".text", build.layout.entry.get(), text_len)?;
    let map = if build.target.triple.cpu == CpuFamily::M68k {
        image.map.clone()
    } else {
        build_output_map(build, program, text_len, &image.symbols)?
    };
    package_generated_linked(
        build,
        program,
        image.bytes,
        map,
        image.symbols,
        image.sections,
    )
}

fn package_generated_linked(
    build: &BuildRequest,
    program: &Program,
    machine_code: Vec<u8>,
    map: String,
    symbols: Vec<AssemblySymbol>,
    sections: Vec<ArtifactSectionMeasurement>,
) -> Result<LinkedCompilation, Diagnostic> {
    let mut build = build.clone();
    let mut resident_code = machine_code;
    let has_game_boy_package = build.package_context.game_boy.is_some();
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
    let is_compact_entry_code = !build.target.triple.value.starts_with("agonlight-mos-ez80")
        && sections.iter().all(|section| section.name == ".text");
    if is_compact_entry_code {
        build.package_context.image_kind = crate::package::PackageImageKind::EntryCode;
        if !has_game_boy_package {
            resident_code = compact_text_payload(&build, &resident_code, &sections)?;
        }
    } else {
        build.package_context.image_kind = crate::package::PackageImageKind::LoadImage;
    }
    package_linked(&build, resident_code, map, symbols, sections, Some(program))
}

fn compact_text_payload(
    build: &BuildRequest,
    image: &[u8],
    sections: &[ArtifactSectionMeasurement],
) -> Result<Vec<u8>, Diagnostic> {
    let Some(text) = sections.iter().find(|section| section.name == ".text") else {
        return Ok(image.to_vec());
    };
    let offset = usize::try_from(text.start.saturating_sub(build.layout.load.get()))
        .map_err(|_| Diagnostic::new("text section exceeds host addressable memory"))?;
    let end = offset
        .checked_add(text.size)
        .ok_or_else(|| Diagnostic::new("text section exceeds host addressable memory"))?;
    let Some(payload) = image.get(offset..end) else {
        return Err(Diagnostic::new(
            "text section extends beyond the linked image",
        ));
    };
    Ok(payload.to_vec())
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
    if build.output_format == OutputFormat::NesRom {
        // NES assembly is a complete iNES image. Its `org` directives use
        // CPU/file coordinates, so section placement must not strip the gaps
        // between the header, PRG vectors, and CHR data.
        return link_assembly_program_at(source_path, program, build.layout.load.get(), build);
    }
    let image = link_assembly_program_image(source_path, program, build)?;
    package_linked(
        build,
        image.bytes,
        image.map,
        image.symbols,
        image.sections,
        None,
    )
}

/// Link a preprocessed standalone assembly program as flat code at an explicit
/// base address, then package it with the resolved target settings.
pub fn link_assembly_program_at(
    source_path: &Path,
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
    let map = if build.output_format == OutputFormat::NesRom {
        flat_assembly_map_at(assembled.bytes.len(), &assembled.symbols, base_addr)?
    } else {
        flat_assembly_map(&build.layout, assembled.bytes.len(), &assembled.symbols)?
    };
    let assembled_len = assembled.bytes.len();
    package_linked(
        build,
        assembled.bytes,
        map,
        assembled.symbols,
        vec![ArtifactSectionMeasurement {
            name: ".text".to_owned(),
            start: base_addr,
            end: base_addr.saturating_add(assembled_len as u32),
            size: assembled_len,
        }],
        None,
    )
}

fn package_linked(
    build: &BuildRequest,
    machine_code: Vec<u8>,
    map: String,
    symbols: Vec<AssemblySymbol>,
    sections: Vec<ArtifactSectionMeasurement>,
    program: Option<&Program>,
) -> Result<LinkedCompilation, Diagnostic> {
    let executable = package_executable_with_context(
        &build.package_request(),
        &build.package_context,
        &machine_code,
    )
    .map_err(|error| Diagnostic::new(error.message))?;
    let source_storage = program.map(source_section_sizes).transpose()?;
    let size_report = ArtifactSizeReport::from_sections_with_storage(
        &sections,
        &symbols,
        executable.len(),
        source_storage.as_ref(),
    );
    validate_size_budgets(&size_report, &build.size_budgets)?;
    Ok(LinkedCompilation {
        machine_code,
        map,
        size_report,
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
    sections: Vec<ArtifactSectionMeasurement>,
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
    let linked_program = order_assembly_sections(source_path, &section_bases, &sections);
    let assembled = assemble_program_with_options_at(
        build.assembler_cpu,
        &linked_program,
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
    let sections = placed
        .iter()
        .map(|section| ArtifactSectionMeasurement {
            name: section.name.clone(),
            start: section.start,
            end: section.start.saturating_add(section.bytes.len() as u32),
            size: section.bytes.len(),
        })
        .collect();
    Ok(LinkedImage {
        bytes: assembly_image_bytes(build, &placed)?,
        map: assembly_section_map(&placed, &assembled.symbols),
        symbols: assembled.symbols,
        sections,
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
        if len == 0 {
            continue;
        }
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

fn order_assembly_sections(
    source_path: &Path,
    section_bases: &[(String, u32, usize)],
    sections: &[AssemblySectionSource],
) -> AssemblyProgram {
    let location = SourceLocation {
        file: source_path.to_path_buf(),
        line: 1,
        column: 1,
    };
    let mut ordered = Vec::new();
    let mut used = BTreeMap::<String, bool>::new();
    let mut placed_names = section_bases.iter().collect::<Vec<_>>();
    placed_names.sort_by_key(|(name, start, _)| (*start, name.as_str()));
    for (name, _, size) in placed_names {
        let Some(section) = sections.iter().find(|section| section.name == *name) else {
            continue;
        };
        used.insert(section.name.clone(), true);
        if *size == 0 {
            continue;
        }
        ordered.push(crate::asm::LocatedAssemblyItem {
            location: location.clone(),
            kind: AssemblyItem::Section(section.name.clone()),
        });
        ordered.extend(section.program.items.iter().cloned());
    }
    for section in sections {
        if used.contains_key(&section.name) {
            continue;
        }
        ordered.push(crate::asm::LocatedAssemblyItem {
            location: location.clone(),
            kind: AssemblyItem::Section(section.name.clone()),
        });
        ordered.extend(section.program.items.iter().cloned());
    }
    AssemblyProgram { items: ordered }
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
        .filter(|section| section.name != ".bss" && !section.bytes.is_empty())
        .map(|section| section.start + section.bytes.len() as u32)
        .max()
        .unwrap_or(build.layout.load.get());
    let len = usize::try_from(max_end.saturating_sub(build.layout.load.get()))
        .map_err(|_| Diagnostic::new("assembly image exceeds host addressable memory"))?;
    let mut image = vec![0; len];
    for section in sections {
        if section.name == ".bss" || section.bytes.is_empty() {
            continue;
        }
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

fn flat_assembly_map(
    layout: &Layout,
    code_len: usize,
    symbols: &[AssemblySymbol],
) -> Result<String, Diagnostic> {
    flat_assembly_map_at(code_len, symbols, layout.entry.get())
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
    matches!(
        build.output_format,
        OutputFormat::CpmCom | OutputFormat::Ez180nGaem
    ) || (build.target.triple.cpu == CpuFamily::I8086
        && build.output_format == OutputFormat::RawBin)
        || bare_target_cpu(&build.target.triple.value).is_some()
        || build.target.triple.value.starts_with("zxspectrum-z80")
        || build.target.triple.value.starts_with("gameboy-")
        || build.target.triple.value.starts_with("arduboy-")
        || build.target.triple.value.starts_with("commodore64-6502")
        || build.target.triple.value.starts_with("nes-")
        || build.target.triple.value == "sega-master-system-z80"
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
    let mut assembly_options = assembly_options_for_target(
        &request.target,
        target.triple.cpu,
        request.debug_comments,
        request.default_sdk_symbols,
    );
    assembly_options.optimization = request.optimization.clone();
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
    assembly_options_for_layout(&layout, cpu, target, debug_comments, default_sdk_symbols)
}

/// Build target assembly options from an explicit layout.
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
    let is_16_bit = crate::target::memory_model_for_cpu(cpu)
        .is_some_and(|memory| memory.address_width_bits <= 16);
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

/// Build target assembly options from an explicit layout and resolved program.
pub fn assembly_options_for_layout_and_program(
    layout: &Layout,
    program: &Program,
    cpu: CpuFamily,
    target: &str,
    debug_comments: bool,
    default_sdk_symbols: bool,
    gameboy_banking: Option<GameBoyBankingOptions>,
) -> Result<AssemblyOptions, Diagnostic> {
    let mut options =
        assembly_options_for_layout(layout, cpu, target, debug_comments, default_sdk_symbols);
    options.gameboy_banking = gameboy_banking;
    options.section_bases = layout_section_bases(program, layout)?;
    Ok(options)
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
    let text_len = assembled
        .section_ranges
        .iter()
        .find(|section| section.name == ".text")
        .map(|section| section.end.saturating_sub(section.start))
        .unwrap_or_else(|| assembled.bytes.len() as u32);
    validate_text_section_fit(layout, text_len as usize)
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
        CpuFamily::M6809 => {
            #[cfg(feature = "m6809")]
            {
                emit_m6809_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "m6809"))]
            {
                Err(Diagnostic::new(
                    "M6809 source compilation requires the `m6809` Cargo feature",
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
        CpuFamily::Avr => {
            #[cfg(feature = "avr")]
            {
                emit_avr_assembly_with_options(program, options)
            }
            #[cfg(not(feature = "avr"))]
            {
                Err(Diagnostic::new(
                    "AVR source compilation requires the `avr` Cargo feature",
                ))
            }
        }
        _ => emit_ez80_assembly_with_options(program, options),
    }
}

/// Resolve the source path used by a compilation request relative to a host
/// application without requiring it to exist on disk.
pub fn source_path(root: impl AsRef<Path>, relative: impl AsRef<Path>) -> PathBuf {
    root.as_ref().join(relative)
}

#[cfg(test)]
mod tests;
